// Payment outbox records, provider event application, order settlement, cancellation, and refund state.

#[allow(clippy::too_many_arguments)]
async fn insert_outbox(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    aggregate_type: &'static str,
    aggregate_id: Uuid,
    event_type: &'static str,
    amount_minor: i64,
    currency: CurrencyCode,
    return_url: Option<&str>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO integration.event_outbox \
         (id, store_id, aggregate_type, aggregate_id, event_type, payload) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(aggregate_type)
    .bind(aggregate_id)
    .bind(event_type)
    .bind(json!({
        "aggregate_id": aggregate_id,
        "amount_minor": amount_minor,
        "currency": currency.as_str(),
        "return_url": return_url,
    }))
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn load_attempt(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    channel_id: Option<SalesChannelId>,
    shopper_id: Option<Uuid>,
    attempt_id: PaymentAttemptId,
) -> Result<Option<PaymentAttemptDetail>, ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            OffsetDateTime,
            OffsetDateTime,
        ),
    >(
        "SELECT attempt.id, attempt.order_id, attempt.amount_minor, \
                attempt.currency::text, attempt.status::text, \
                attempt.stripe_checkout_session_id, attempt.failure_code, \
                attempt.created_at, attempt.updated_at \
         FROM commerce.payment_attempts AS attempt \
         INNER JOIN commerce.orders AS sales_order \
           ON sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
         WHERE attempt.store_id = $1 AND attempt.id = $2 \
           AND ($3::uuid IS NULL OR sales_order.sales_channel_id = $3) \
           AND ($4::uuid IS NULL OR sales_order.shopper_id = $4)",
    )
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .bind(channel_id.map(SalesChannelId::as_uuid))
    .bind(shopper_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        Ok(PaymentAttemptDetail {
            id: PaymentAttemptId::from_uuid(row.0),
            order_id: OrderId::from_uuid(row.1),
            amount_minor: row.2,
            currency: CurrencyCode::parse(&row.3)?,
            status: PaymentAttemptStatus::parse(&row.4).ok_or_else(corrupt_payment_state)?,
            stripe_checkout_session_id: row.5,
            failure_code: row.6,
            created_at: row.7,
            updated_at: row.8,
        })
    })
    .transpose()
}

/// Updates the Order's `payment_status` summary only when it still matches
/// one of `from_statuses`, so an out-of-order or replayed webhook cannot
/// clobber a state that has already moved on (e.g. a late `payment.captured`
/// arriving after the Order was already refunded).
async fn update_order_payment_status(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    from_statuses: &[&str],
    to_status: &str,
    now: OffsetDateTime,
) -> Result<bool, ApplicationError> {
    let rows = sqlx::query(
        "UPDATE commerce.orders SET payment_status = $3::commerce.order_payment_status, \
                updated_at = $4 \
         WHERE store_id = $1 AND id = $2 AND payment_status::text = ANY($5)",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(to_status)
    .bind(now)
    .bind(from_statuses)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?
    .rows_affected();
    Ok(rows == 1)
}

/// Recomputes `refunded_amount_minor` and `payment_status` from the
/// authoritative `commerce.refunds` rows for this Order. Summing from source
/// on every call makes replayed refund webhooks naturally idempotent instead
/// of relying on an incrementally patched running total.
async fn recompute_order_refund_summary(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let (refunded, total): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE((SELECT SUM(refund.amount_minor) FROM commerce.refunds AS refund \
                           WHERE refund.store_id = sales_order.store_id \
                             AND refund.order_id = sales_order.id \
                             AND refund.status = 'succeeded'), 0), \
                sales_order.total_amount_minor \
         FROM commerce.orders AS sales_order \
         WHERE sales_order.store_id = $1 AND sales_order.id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    let payment_status = if refunded >= total {
        "refunded"
    } else if refunded > 0 {
        "partially_refunded"
    } else {
        "paid"
    };
    sqlx::query(
        "UPDATE commerce.orders SET refunded_amount_minor = $3, \
                payment_status = $4::commerce.order_payment_status, updated_at = $5 \
         WHERE store_id = $1 AND id = $2 \
           AND payment_status IN ('paid', 'partially_refunded', 'refunded')",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(refunded)
    .bind(payment_status)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_payment_event(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    attempt_id: PaymentAttemptId,
    event_type: &str,
    stripe_checkout_session_id: String,
    failure_code: Option<String>,
    provider_payload: &Value,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let attempt_order_id: Uuid = sqlx::query_scalar(
        "SELECT order_id FROM commerce.payment_attempts \
         WHERE store_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| attempt_not_found(attempt_id))?;
    let order_id = OrderId::from_uuid(attempt_order_id);
    let captured = matches!(event_type, "payment.authorized" | "payment.captured");
    let failed = matches!(event_type, "payment.failed" | "payment.cancelled");
    if !captured && !failed {
        return Err(corrupt_webhook_payload());
    }
    let (order_status, shopper_id, currency): (String, Uuid, String) = sqlx::query_as(
        "SELECT status::text, shopper_id, currency::text \
         FROM commerce.orders WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;

    if captured {
        let applied = sqlx::query(
            "UPDATE commerce.payment_attempts SET status = 'captured', failure_code = NULL, \
                    stripe_checkout_session_id = COALESCE($3, stripe_checkout_session_id), \
                    updated_at = $4 \
             WHERE store_id = $1 AND id = $2 AND status <> 'captured'",
        )
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(&stripe_checkout_session_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?
        .rows_affected()
            == 1;
        if !applied {
            return Ok(());
        }
        let mut event_amount: i64 = sqlx::query_scalar(
            "SELECT amount_minor FROM commerce.payment_attempts WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?;
        if let Some(snapshot) = StripeCheckoutSnapshot::from_payload(provider_payload)? {
            event_amount = apply_stripe_checkout_snapshot(
                transaction, store_id, order_id, attempt_id, &snapshot, now,
            )
            .await?;
        }
        update_order_payment_status(
            transaction,
            store_id,
            order_id,
            &["pending", "failed"],
            "paid",
            now,
        )
        .await?;
        confirm_paid_order(transaction, store_id, order_id, &order_status, now).await?;
        append_event(
            transaction,
            AnalyticsEventToAppend {
                store_id: store_id.as_uuid(),
                shopper_id,
                event_id: attempt_id.as_uuid(),
                event_name: "purchase".into(),
                properties: json!({
                    "_source": "server",
                    "order_id": order_id.as_uuid(),
                    "payment_attempt_id": attempt_id.as_uuid(),
                    "value_minor": event_amount,
                    "currency": currency,
                }),
                occurred_at: now,
                received_at: now,
            },
        )
        .await?;
    } else if failed {
        let applied = sqlx::query(
            "UPDATE commerce.payment_attempts SET status = 'failed', failure_code = $3, \
                    stripe_checkout_session_id = COALESCE($4, stripe_checkout_session_id), \
                    updated_at = $5 \
             WHERE store_id = $1 AND id = $2 AND status = 'pending'",
        )
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(failure_code.unwrap_or_else(|| "provider_failure".into()))
        .bind(&stripe_checkout_session_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?
        .rows_affected()
            == 1;
        if !applied {
            return Ok(());
        }
        update_order_payment_status(transaction, store_id, order_id, &["pending"], "failed", now)
            .await?;
        cancel_pending_order(transaction, store_id, order_id, &order_status, now).await?;
    }
    Ok(())
}

struct StripeCheckoutSnapshot {
    amount_subtotal: i64,
    amount_discount: i64,
    amount_tax: i64,
    amount_shipping: i64,
    amount_total: i64,
    currency: String,
    payment_intent_id: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    billing_address: Option<StripeAddressSnapshot>,
    shipping_address: Option<StripeAddressSnapshot>,
}

struct StripeAddressSnapshot {
    full_name: String,
    line1: String,
    line2: Option<String>,
    city: String,
    state: Option<String>,
    postal_code: Option<String>,
    country: String,
}

impl StripeCheckoutSnapshot {
    fn from_payload(payload: &Value) -> Result<Option<Self>, ApplicationError> {
        let Some(object) = payload
            .get("stripe_event")
            .and_then(|event| event.get("data"))
            .and_then(|data| data.get("object"))
        else {
            return Ok(None);
        };
        let amount_total = object
            .get("amount_total")
            .and_then(Value::as_i64)
            .ok_or_else(corrupt_webhook_payload)?;
        let amount_subtotal = object
            .get("amount_subtotal")
            .and_then(Value::as_i64)
            .unwrap_or(amount_total);
        let amount_tax = object
            .get("total_details")
            .and_then(|details| details.get("amount_tax"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let amount_discount = object
            .get("total_details")
            .and_then(|details| details.get("amount_discount"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let amount_shipping = object
            .get("shipping_cost")
            .and_then(|shipping| shipping.get("amount_total"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let currency = object
            .get("currency")
            .and_then(Value::as_str)
            .ok_or_else(corrupt_webhook_payload)?
            .to_ascii_uppercase();
        let payment_intent_id = object
            .get("payment_intent")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if payment_intent_id
            .as_deref()
            .is_some_and(|value| !value.starts_with("pi_"))
        {
            return Err(corrupt_webhook_payload());
        }
        if amount_total <= 0
            || amount_subtotal < 0
            || amount_discount < 0
            || amount_tax < 0
            || amount_shipping < 0
        {
            return Err(corrupt_webhook_payload());
        }
        let customer_details = object.get("customer_details");
        let shipping_details = object.get("shipping_details");
        let email = customer_details
            .and_then(|value| value.get("email"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let phone = customer_details
            .and_then(|value| value.get("phone"))
            .and_then(Value::as_str)
            .filter(|value| valid_e164(value))
            .map(str::to_owned);
        let billing_address = customer_details
            .and_then(|value| value.get("address"))
            .and_then(|address| {
                stripe_address(
                    address,
                    customer_details
                        .and_then(|value| value.get("name"))
                        .and_then(Value::as_str),
                )
                .transpose()
            })
            .transpose()?;
        let shipping_address = shipping_details
            .and_then(|value| value.get("address"))
            .and_then(|address| {
                stripe_address(
                    address,
                    shipping_details
                        .and_then(|value| value.get("name"))
                        .and_then(Value::as_str),
                )
                .transpose()
            })
            .transpose()?;
        Ok(Some(Self {
            amount_subtotal,
            amount_discount,
            amount_tax,
            amount_shipping,
            amount_total,
            currency,
            payment_intent_id,
            email,
            phone,
            billing_address,
            shipping_address,
        }))
    }
}

fn stripe_address(
    value: &Value,
    name: Option<&str>,
) -> Result<Option<StripeAddressSnapshot>, ApplicationError> {
    let Some(line1) = value.get("line1").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(city) = value.get("city").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(country) = value.get("country").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(full_name) = name.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let country = country.to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(corrupt_webhook_payload());
    }
    Ok(Some(StripeAddressSnapshot {
        full_name: full_name.to_owned(),
        line1: line1.to_owned(),
        line2: value
            .get("line2")
            .and_then(Value::as_str)
            .map(str::to_owned),
        city: city.to_owned(),
        state: value
            .get("state")
            .and_then(Value::as_str)
            .map(str::to_owned),
        postal_code: value
            .get("postal_code")
            .and_then(Value::as_str)
            .map(str::to_owned),
        country,
    }))
}

fn valid_e164(value: &str) -> bool {
    let bytes = value.as_bytes();
    (9..=16).contains(&bytes.len())
        && bytes.first() == Some(&b'+')
        && bytes.get(1).is_some_and(|byte| *byte != b'0')
        && bytes[1..].iter().all(u8::is_ascii_digit)
}

async fn apply_stripe_checkout_snapshot(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    attempt_id: PaymentAttemptId,
    snapshot: &StripeCheckoutSnapshot,
    now: OffsetDateTime,
) -> Result<i64, ApplicationError> {
    let current_currency: String = sqlx::query_scalar(
        "SELECT currency::text FROM commerce.orders WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if current_currency != snapshot.currency {
        return Err(stripe_currency_mismatch());
    }
    sqlx::query(
        "UPDATE commerce.orders SET subtotal_amount_minor = $3, discount_amount_minor = $4, \
                tax_amount_minor = $5, shipping_amount_minor = $6, total_amount_minor = $7, \
                updated_at = $8 WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(snapshot.amount_subtotal)
    .bind(snapshot.amount_discount)
    .bind(snapshot.amount_tax)
    .bind(snapshot.amount_shipping)
    .bind(snapshot.amount_total)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if let Some(payment_intent_id) = snapshot.payment_intent_id.as_deref() {
        sqlx::query(
            "UPDATE commerce.payment_attempts \
             SET stripe_payment_intent_id = COALESCE(stripe_payment_intent_id, $3), \
                 updated_at = $4 \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(payment_intent_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    if let Some(email) = snapshot.email.as_deref() {
        sqlx::query(
            "UPDATE commerce.orders SET contact_email = $3, updated_at = $4 \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(email)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    if let Some(phone) = snapshot.phone.as_deref() {
        sqlx::query(
            "UPDATE commerce.orders SET contact_phone = $3, updated_at = $4 \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(phone)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    if let Some(address) = snapshot.billing_address.as_ref() {
        update_inline_address(transaction, store_id, order_id, "billing", address, now).await?;
    }
    if let Some(address) = snapshot.shipping_address.as_ref() {
        update_inline_address(transaction, store_id, order_id, "shipping", address, now).await?;
    }
    Ok(snapshot.amount_total)
}

async fn update_inline_address(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    kind: &str,
    address: &StripeAddressSnapshot,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let (full_name, line1, line2, locality, area, postal_code, country) = (
        &address.full_name,
        &address.line1,
        &address.line2,
        &address.city,
        &address.state,
        &address.postal_code,
        &address.country,
    );
    let query = match kind {
        "billing" => "UPDATE commerce.orders SET billing_full_name=$3, billing_address_line1=$4, billing_address_line2=$5, billing_locality=$6, billing_administrative_area=$7, billing_postal_code=$8, billing_country_code=$9, updated_at=$10 WHERE store_id=$1 AND id=$2",
        "shipping" => "UPDATE commerce.orders SET shipping_full_name=$3, shipping_address_line1=$4, shipping_address_line2=$5, shipping_locality=$6, shipping_administrative_area=$7, shipping_postal_code=$8, shipping_country_code=$9, updated_at=$10 WHERE store_id=$1 AND id=$2",
        _ => return Err(corrupt_webhook_payload()),
    };
    sqlx::query(query)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(full_name)
        .bind(line1)
        .bind(line2)
        .bind(locality)
        .bind(area)
        .bind(postal_code)
        .bind(country)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(())
}

async fn cancel_pending_order(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    status: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let status = OrderStatus::parse(status).ok_or_else(corrupt_payment_state)?;
    if status != OrderStatus::Pending {
        return Ok(());
    }
    let mut order = Order::rehydrate(order_id, status);
    let transition = order.cancel(now)?;
    sqlx::query(
        "UPDATE commerce.orders SET status = 'cancelled', updated_at = $3 \
         WHERE store_id = $1 AND id = $2 AND status = 'pending'",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO commerce.order_transitions \
         (id, store_id, order_id, from_status, to_status, kind, occurred_at) \
         VALUES ($1, $2, $3, $4::commerce.order_status, 'cancelled', 'cancelled', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(transition.from_status.map(OrderStatus::as_str))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn confirm_paid_order(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    status: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let status = OrderStatus::parse(status).ok_or_else(corrupt_payment_state)?;
    if status == OrderStatus::Confirmed {
        return Ok(());
    }
    let mut order = Order::rehydrate(order_id, status);
    let transition = order.confirm(now)?;
    consume_order_inventory(transaction, store_id, order_id).await?;
    sqlx::query(
        "UPDATE commerce.orders SET status = 'confirmed', updated_at = $3 \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    let cart_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT cart_id FROM commerce.orders WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    if let Some(cart_id) = cart_id {
        sqlx::query(
            "UPDATE commerce.carts SET status = 'completed', version = version + 1, updated_at = $3 \
             WHERE store_id = $1 AND id = $2 AND status = 'active'",
        )
        .bind(store_id.as_uuid())
        .bind(cart_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    let transition_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO commerce.order_transitions \
         (id, store_id, order_id, from_status, to_status, kind, occurred_at) \
         VALUES ($1, $2, $3, $4::commerce.order_status, 'confirmed', 'confirmed', $5)",
    )
    .bind(transition_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(transition.from_status.map(OrderStatus::as_str))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    let (_, tracking_digest) = generate_order_tracking_token();
    sqlx::query(
        "INSERT INTO commerce.order_tracking_tokens \
         (store_id,order_id,token_digest,expires_at,created_at) \
         VALUES($1,$2,$3,$4,$5) ON CONFLICT(store_id,order_id) DO NOTHING",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(tracking_digest.as_slice())
    .bind(now + ORDER_TRACKING_TOKEN_LIFETIME)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn consume_order_inventory(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    let lines = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT product_variant_id, quantity::bigint \
         FROM commerce.order_lines \
         WHERE store_id = $1 AND order_id = $2 AND track_inventory \
         ORDER BY product_variant_id",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    for (product_variant_id, quantity) in lines {
        sqlx::query_scalar::<_, i64>(
            "UPDATE commerce.product_variants \
             SET on_hand_quantity = on_hand_quantity - $3, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2 AND track_inventory \
               AND on_hand_quantity >= $3 \
             RETURNING on_hand_quantity",
        )
        .bind(store_id.as_uuid())
        .bind(product_variant_id)
        .bind(quantity)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ApplicationError::Conflict {
            code: "insufficient_inventory",
            message: "one or more order lines exceed available inventory",
        })?;
    }
    Ok(())
}

/// Applies a `refund.` provider event. `refund_id` is `Some` for a refund
/// Chaos itself initiated (via `create_refund`), whose row already exists.
/// It is `None` for a refund created outside Chaos (e.g. from the Stripe
/// Dashboard) — that case is resolved through the PaymentIntent reference
/// and the Refund row is created here, on first sight, with no
/// `created_by_user_id`.
#[allow(clippy::too_many_arguments)]
async fn apply_refund_event(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    refund_id: Option<RefundId>,
    event_type: &str,
    stripe_refund_id: String,
    failure_code: Option<String>,
    provider_payload: &Value,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let object = provider_payload
        .get("stripe_event")
        .and_then(|event| event.get("data"))
        .and_then(|data| data.get("object"))
        .ok_or_else(corrupt_webhook_payload)?;
    let amount = object
        .get("amount")
        .and_then(Value::as_i64)
        .ok_or_else(corrupt_webhook_payload)?;
    let payment_intent = object
        .get("payment_intent")
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("pi_"));
    if amount <= 0 {
        return Err(corrupt_webhook_payload());
    }
    let succeeded = event_type == "refund.succeeded";
    let failed = event_type == "refund.failed";
    if !succeeded && !failed {
        return Err(corrupt_webhook_payload());
    }

    let resolved: Option<(Uuid, Uuid, Uuid)> = match refund_id {
        Some(id) => sqlx::query_as(
            "SELECT id, payment_attempt_id, order_id FROM commerce.refunds \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?,
        None => None,
    };

    let (refund_row_id, payment_attempt_id, order_id) = match resolved {
        Some(row) => row,
        None => {
            let payment_intent = payment_intent.ok_or_else(corrupt_webhook_payload)?;
            let attempt: (Uuid, Uuid, String) = sqlx::query_as(
                "SELECT id, order_id, currency::text FROM commerce.payment_attempts \
                 WHERE store_id = $1 AND stripe_payment_intent_id = $2 FOR UPDATE",
            )
            .bind(store_id.as_uuid())
            .bind(payment_intent)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?;
            sqlx::query(
                "INSERT INTO commerce.refunds \
                 (id, store_id, payment_attempt_id, order_id, currency, status, amount_minor, \
                  stripe_refund_id) \
                 VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7) \
                 ON CONFLICT (store_id, stripe_refund_id) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(store_id.as_uuid())
            .bind(attempt.0)
            .bind(attempt.1)
            .bind(&attempt.2)
            .bind(amount)
            .bind(&stripe_refund_id)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query_as(
                "SELECT id, payment_attempt_id, order_id FROM commerce.refunds \
                 WHERE store_id = $1 AND stripe_refund_id = $2 FOR UPDATE",
            )
            .bind(store_id.as_uuid())
            .bind(&stripe_refund_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(database_error)?
        }
    };

    if succeeded {
        let applied = sqlx::query(
            "UPDATE commerce.refunds SET status = 'succeeded', stripe_refund_id = $3, \
                    failure_code = NULL, updated_at = $4 \
             WHERE store_id = $1 AND id = $2 AND status <> 'succeeded'",
        )
        .bind(store_id.as_uuid())
        .bind(refund_row_id)
        .bind(&stripe_refund_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?
        .rows_affected()
            == 1;
        if !applied {
            return Ok(());
        }
        recompute_order_refund_summary(transaction, store_id, OrderId::from_uuid(order_id), now)
            .await?;
        let shopper_id: Uuid = sqlx::query_scalar(
            "SELECT shopper_id FROM commerce.orders WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(order_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?;
        append_event(
            transaction,
            AnalyticsEventToAppend {
                store_id: store_id.as_uuid(),
                shopper_id,
                event_id: refund_row_id,
                event_name: "refund".into(),
                properties: json!({
                    "_source": "server",
                    "refund_id": refund_row_id,
                    "payment_attempt_id": payment_attempt_id,
                    "order_id": order_id,
                    "value_minor": amount,
                }),
                occurred_at: now,
                received_at: now,
            },
        )
        .await?;
    } else if failed {
        sqlx::query(
            "UPDATE commerce.refunds SET status = 'failed', stripe_refund_id = $3, \
                    failure_code = $4, updated_at = $5 \
             WHERE store_id = $1 AND id = $2 AND status = 'pending'",
        )
        .bind(store_id.as_uuid())
        .bind(refund_row_id)
        .bind(&stripe_refund_id)
        .bind(failure_code.unwrap_or_else(|| "provider_failure".into()))
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}
