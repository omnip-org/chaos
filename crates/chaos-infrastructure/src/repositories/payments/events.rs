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
                attempt.currency::text, attempt.status::text, attempt.stripe_checkout_session_id, \
                attempt.failure_code, attempt.created_at, attempt.updated_at \
         FROM commerce.payment_attempts AS attempt \
         INNER JOIN commerce.orders AS sales_order \
           ON sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
         WHERE attempt.store_id = $1 AND attempt.id = $2 \
           AND ($3::uuid IS NULL OR sales_order.sales_channel_id = $3) \
           AND ($4::uuid IS NULL OR attempt.shopper_id = $4)",
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

async fn load_refund(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    refund_id: RefundId,
) -> Result<Option<RefundDetail>, ApplicationError> {
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
        "SELECT id, payment_attempt_id, amount_minor, currency::text, status::text, \
                stripe_refund_id, failure_code, created_at, updated_at \
         FROM commerce.refunds WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(refund_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        Ok(RefundDetail {
            id: RefundId::from_uuid(row.0),
            payment_attempt_id: PaymentAttemptId::from_uuid(row.1),
            amount_minor: row.2,
            currency: CurrencyCode::parse(&row.3)?,
            status: RefundStatus::parse(&row.4).ok_or_else(corrupt_payment_state)?,
            stripe_refund_id: row.5,
            failure_code: row.6,
            created_at: row.7,
            updated_at: row.8,
        })
    })
    .transpose()
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
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            i64,
            String,
            String,
            Option<String>,
            Option<Uuid>,
            String,
            Uuid,
        ),
    >(
        "SELECT attempt.order_id, attempt.amount_minor, attempt.currency::text, \
                attempt.status::text, attempt.stripe_checkout_session_id, \
                sales_order.inventory_reservation_id, sales_order.status::text, \
                sales_order.shopper_id \
         FROM commerce.payment_attempts AS attempt \
         INNER JOIN commerce.orders AS sales_order \
           ON sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
         WHERE attempt.store_id = $1 AND attempt.id = $2 \
         FOR UPDATE OF attempt, sales_order",
    )
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| attempt_not_found(attempt_id))?;
    let currency = CurrencyCode::parse(&row.2)?;
    let mut attempt = PaymentAttempt::rehydrate(
        attempt_id,
        OrderId::from_uuid(row.0),
        Money::new(row.1, currency),
        PaymentAttemptStatus::parse(&row.3).ok_or_else(corrupt_payment_state)?,
        row.4,
    );
    let changed = match event_type {
        "payment.authorized" => attempt.authorize(stripe_checkout_session_id)?,
        "payment.captured" => {
            if attempt.status() == PaymentAttemptStatus::Pending {
                attempt.authorize(stripe_checkout_session_id.clone())?;
            } else if attempt.stripe_checkout_session_id()
                != Some(stripe_checkout_session_id.as_str())
            {
                return Err(stripe_object_mismatch());
            }
            attempt.capture()?
        }
        "payment.failed" => attempt.fail(Some(stripe_checkout_session_id))?,
        "payment.cancelled" => attempt.cancel(Some(stripe_checkout_session_id))?,
        _ => return Err(corrupt_webhook_payload()),
    };
    if !changed {
        return Ok(());
    }
    let stored_failure = if attempt.status() == PaymentAttemptStatus::Failed {
        Some(failure_code.unwrap_or_else(|| "provider_failure".into()))
    } else {
        None
    };
    sqlx::query(
        "UPDATE commerce.payment_attempts \
         SET status = $3::commerce.payment_attempt_status, stripe_checkout_session_id = $4, \
             failure_code = $5, updated_at = $6 \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .bind(attempt.status().as_str())
    .bind(attempt.stripe_checkout_session_id())
    .bind(stored_failure)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    let mut event_amount = row.1;
    if attempt.status() == PaymentAttemptStatus::Captured {
        if let Some(snapshot) = StripeCheckoutSnapshot::from_payload(provider_payload)? {
            event_amount = apply_stripe_checkout_snapshot(
                transaction,
                store_id,
                attempt_id,
                OrderId::from_uuid(row.0),
                &snapshot,
                now,
            )
            .await?;
        }
        confirm_paid_order(
            transaction,
            store_id,
            OrderId::from_uuid(row.0),
            row.5.map(InventoryReservationId::from_uuid),
            &row.6,
            now,
        )
        .await?;
        append_event(
            transaction,
            AnalyticsEventToAppend {
                store_id: store_id.as_uuid(),
                shopper_id: row.7,
                event_id: row.0,
                event_name: "purchase".into(),
                properties: json!({
                    "_source": "server",
                    "order_id": row.0,
                    "payment_attempt_id": attempt_id.as_uuid(),
                    "value_minor": event_amount,
                    "currency": row.2,
                }),
                occurred_at: now,
                received_at: now,
            },
        )
        .await?;
    } else if matches!(
        attempt.status(),
        PaymentAttemptStatus::Failed | PaymentAttemptStatus::Cancelled
    ) {
        cancel_pending_order(
            transaction,
            store_id,
            OrderId::from_uuid(row.0),
            row.5.map(InventoryReservationId::from_uuid),
            &row.6,
            now,
        )
        .await?;
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
    attempt_id: PaymentAttemptId,
    order_id: OrderId,
    snapshot: &StripeCheckoutSnapshot,
    now: OffsetDateTime,
) -> Result<i64, ApplicationError> {
    let current_currency: String = sqlx::query_scalar(
        "SELECT currency::text FROM commerce.payment_attempts WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if current_currency != snapshot.currency {
        return Err(stripe_currency_mismatch());
    }
    sqlx::query(
        "UPDATE commerce.payment_attempts SET amount_minor = $3, \
                stripe_payment_intent_id = COALESCE($4, stripe_payment_intent_id), \
                updated_at = $5 \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .bind(snapshot.amount_total)
    .bind(snapshot.payment_intent_id.as_deref())
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
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
    let country = snapshot
        .shipping_address
        .as_ref()
        .or(snapshot.billing_address.as_ref())
        .map(|address| address.country.as_str())
        .ok_or_else(corrupt_webhook_payload)?;
    if snapshot.amount_shipping > 0
        && let Some(service) = sqlx::query_as::<_, (Uuid, String, String, String, i16, i16)>(
            "SELECT service.id, service.code, service.name, service.currency::text, \
                    service.estimated_min_days, service.estimated_max_days \
             FROM commerce.shipping_services AS service \
             INNER JOIN commerce.shipping_service_regions AS region \
               ON region.store_id = service.store_id AND region.shipping_service_id = service.id \
             WHERE service.store_id = $1 AND service.currency = $2 AND service.status = 'active' \
               AND region.country_code = $3 ORDER BY abs(service.amount_minor - $4), service.id LIMIT 1",
        )
        .bind(store_id.as_uuid())
        .bind(&snapshot.currency)
        .bind(country)
        .bind(snapshot.amount_shipping)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?
        {
        sqlx::query(
                "INSERT INTO commerce.order_shipping_selections \
                 (store_id, order_id, shipping_service_id, service_code, service_name, amount_minor, currency, estimated_min_days, estimated_max_days) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
                 ON CONFLICT (store_id, order_id) DO UPDATE SET shipping_service_id=EXCLUDED.shipping_service_id, \
                   service_code=EXCLUDED.service_code, service_name=EXCLUDED.service_name, amount_minor=EXCLUDED.amount_minor, \
                   currency=EXCLUDED.currency, estimated_min_days=EXCLUDED.estimated_min_days, estimated_max_days=EXCLUDED.estimated_max_days",
            )
            .bind(store_id.as_uuid())
            .bind(order_id.as_uuid())
            .bind(service.0)
            .bind(&service.1)
            .bind(&service.2)
            .bind(snapshot.amount_shipping)
            .bind(&service.3)
            .bind(service.4)
            .bind(service.5)
            .execute(&mut **transaction)
            .await
        .map_err(database_error)?;
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
    reservation_id: Option<InventoryReservationId>,
    status: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let status = OrderStatus::parse(status).ok_or_else(corrupt_payment_state)?;
    if status != OrderStatus::Pending {
        return Ok(());
    }
    let mut order = Order::rehydrate(order_id, status);
    let transition = order.cancel(now)?;
    if let Some(reservation_id) = reservation_id {
        let active: Option<bool> = sqlx::query_scalar(
            "SELECT status = 'active' FROM commerce.inventory_reservations \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(reservation_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
        if active == Some(true) {
            close_reservation(
                transaction,
                store_id,
                reservation_id,
                ReservationClosure::Released,
                now,
            )
            .await?;
        }
    }
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
    reservation_id: Option<InventoryReservationId>,
    status: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let status = OrderStatus::parse(status).ok_or_else(corrupt_payment_state)?;
    if status == OrderStatus::Confirmed {
        return Ok(());
    }
    let mut order = Order::rehydrate(order_id, status);
    let transition = order.confirm(now)?;
    if let Some(reservation_id) = reservation_id {
        close_reservation(
            transaction,
            store_id,
            reservation_id,
            ReservationClosure::Consumed,
            now,
        )
        .await?;
    }
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
    let (_, tracking_digest) = generate_order_tracking_key();
    sqlx::query(
        "INSERT INTO commerce.order_tracking_keys \
         (id,store_id,order_id,secret_digest,expires_at,created_at) \
         VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(store_id,order_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(tracking_digest.as_slice())
    .bind(now + ORDER_TRACKING_KEY_LIFETIME)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_refund_event(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    refund_id: RefundId,
    event_type: &str,
    stripe_refund_id: String,
    failure_code: Option<String>,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, i64, String, String, Option<String>, Uuid, Uuid)>(
        "SELECT refund.payment_attempt_id, refund.amount_minor, refund.currency::text,
                refund.status::text, refund.stripe_refund_id, attempt.order_id, order_row.shopper_id \
         FROM commerce.refunds refund
         JOIN commerce.payment_attempts attempt
           ON attempt.store_id=refund.store_id AND attempt.id=refund.payment_attempt_id
         JOIN commerce.orders order_row
           ON order_row.store_id=attempt.store_id AND order_row.id=attempt.order_id
         WHERE refund.store_id = $1 AND refund.id = $2 \
         FOR UPDATE OF refund, attempt, order_row",
    )
    .bind(store_id.as_uuid())
    .bind(refund_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| refund_not_found(refund_id))?;
    let mut refund = Refund::rehydrate(
        refund_id,
        PaymentAttemptId::from_uuid(row.0),
        Money::new(row.1, CurrencyCode::parse(&row.2)?),
        RefundStatus::parse(&row.3).ok_or_else(corrupt_payment_state)?,
        row.4,
    );
    let changed = match event_type {
        "refund.succeeded" => refund.succeed(stripe_refund_id)?,
        "refund.failed" => refund.fail(stripe_refund_id)?,
        _ => return Err(corrupt_webhook_payload()),
    };
    if !changed {
        return Ok(());
    }
    let stored_failure = if refund.status() == RefundStatus::Failed {
        Some(failure_code.unwrap_or_else(|| "provider_failure".into()))
    } else {
        None
    };
    sqlx::query(
        "UPDATE commerce.refunds \
         SET status = $3::commerce.refund_status, stripe_refund_id = $4, \
             failure_code = $5, updated_at = $6 \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(refund_id.as_uuid())
    .bind(refund.status().as_str())
    .bind(refund.stripe_refund_id())
    .bind(stored_failure)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if refund.status() == RefundStatus::Succeeded {
        append_event(
            transaction,
            AnalyticsEventToAppend {
                store_id: store_id.as_uuid(),
                shopper_id: row.6,
                event_id: refund_id.as_uuid(),
                event_name: "refund".into(),
                properties: json!({
                    "_source": "server",
                    "refund_id": refund_id.as_uuid(),
                    "payment_attempt_id": row.0,
                    "order_id": row.5,
                    "value_minor": row.1,
                    "currency": row.2,
                }),
                occurred_at: now,
                received_at: now,
            },
        )
        .await?;
    }
    Ok(())
}
