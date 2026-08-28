// Payment outbox records, provider event application, order settlement, cancellation, and refund state.

type RefundDetailRow = (
    Uuid,
    String,
    i64,
    Option<String>,
    Option<String>,
    OffsetDateTime,
    OffsetDateTime,
);

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
         (id, store_id, aggregate_type, aggregate_id, internal_event_type, payload) \
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
        "provider": "stripe",
    }))
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn insert_order_confirmed_event(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    tracking_token: &secrecy::SecretString,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO integration.event_outbox \
         (id, store_id, aggregate_type, aggregate_id, internal_event_type, payload) \
         VALUES ($1, $2, 'order', $3, 'order.confirmed', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(json!({
        "aggregate_id": order_id.as_uuid(),
        "order_id": order_id.as_uuid(),
        "tracking_token": tracking_token.expose_secret(),
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
    order_id: OrderId,
) -> Result<Option<PaymentAttemptDetail>, ApplicationError> {
    // subtotal_amount_minor is the pre-tax reference amount Chaos already
    // knows when a checkout attempt is created; total_amount_minor is a
    // Stripe-reported fact filled in only after the checkout session
    // settles, so it is not yet meaningful here. prepare_payment_command
    // validates a checkout outbox job's amount against this same
    // subtotal_amount_minor column, so both must agree.
    let row = sqlx::query_as::<
        _,
        (
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
        "SELECT sales_order.id, sales_order.subtotal_amount_minor, \
                sales_order.currency::text, sales_order.payment_status::text, \
                sales_order.payment_provider_reference_id, sales_order.payment_failure_code, \
                sales_order.created_at, sales_order.updated_at \
         FROM commerce.orders AS sales_order \
         WHERE sales_order.store_id = $1 AND sales_order.id = $2 \
           AND ($3::uuid IS NULL OR sales_order.sales_channel_id = $3) \
           AND ($4::uuid IS NULL OR sales_order.shopper_id = $4)",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(channel_id.map(SalesChannelId::as_uuid))
    .bind(shopper_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(payment_attempt_detail).transpose()
}

fn payment_attempt_detail(
    row: (
        Uuid,
        i64,
        String,
        String,
        Option<String>,
        Option<String>,
        OffsetDateTime,
        OffsetDateTime,
    ),
) -> Result<PaymentAttemptDetail, ApplicationError> {
    Ok(PaymentAttemptDetail {
        order_id: OrderId::from_uuid(row.0),
        amount_minor: row.1,
        currency: CurrencyCode::parse(&row.2)?,
        status: match row.3.as_str() {
            "pending" => PaymentAttemptStatus::Pending,
            "paid" | "partially_refunded" | "refunded" => PaymentAttemptStatus::Captured,
            "failed" => PaymentAttemptStatus::Failed,
            _ => return Err(corrupt_payment_state()),
        },
        provider_reference_id: row.4,
        failure_code: row.5,
        created_at: row.6,
        updated_at: row.7,
    })
}

/// Updates the Order's `payment_status` summary only when it still matches
/// one of `from_statuses`, so an out-of-order or replayed webhook cannot
/// clobber a state that has already moved on (e.g. a late `payment.captured`
/// arriving after the Order was already refunded). A capture is only valid
/// while the Order is still pending; callers decide whether a no-op is a
/// harmless replay or a terminal out-of-order event.
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
                             AND refund.status = 'succeeded'), 0)::bigint, \
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

async fn load_refund_reconciliation_context(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    provider_account_id: Uuid,
    payment_provider_reference: &str,
) -> Result<Option<RefundReconciliationContext>, ApplicationError> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT sales_order.id, account.credential_secret_reference \
         FROM commerce.orders AS sales_order \
         INNER JOIN integration.provider_accounts AS account \
           ON account.store_id = sales_order.store_id \
          AND account.id = sales_order.payment_provider_account_id \
          AND account.capability = 'payment' \
          AND account.provider = 'stripe' \
          AND account.enabled \
         WHERE sales_order.store_id = $1 \
           AND sales_order.payment_provider_account_id = $2 \
           AND sales_order.payment_provider_reference_id = $3 \
           AND account.credential_secret_reference IS NOT NULL \
         FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(provider_account_id)
    .bind(payment_provider_reference)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(row.map(|(order_id, credential_secret_reference)| {
        RefundReconciliationContext {
            store_id,
            order_id: OrderId::from_uuid(order_id),
            provider_account_id,
            credential_secret_reference,
            payment_provider_reference: payment_provider_reference.to_owned(),
        }
    }))
}

impl PostgresStripeRepository {
    pub(crate) async fn prepare_refund_reconciliation(
        &self,
        actor: &AdminActor,
        store_id: StoreId,
        order_id: OrderId,
    ) -> Result<RefundReconciliationContext, ApplicationError> {
        let mut transaction = self.begin_admin(actor).await?;
        let row: Option<(Uuid, Option<String>)> = sqlx::query_as(
            "SELECT payment_provider_account_id, payment_provider_reference_id \
             FROM commerce.orders \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let (provider_account_id, payment_provider_reference) = row
            .ok_or_else(|| order_not_found(order_id))?;
        let payment_provider_reference = payment_provider_reference.ok_or(ApplicationError::Conflict {
            code: "stripe_payment_intent_missing",
            message: "the Order has no Stripe PaymentIntent",
        })?;
        let context = load_refund_reconciliation_context(
            &mut transaction,
            store_id,
            provider_account_id,
            &payment_provider_reference,
        )
        .await?
        .ok_or_else(provider_unavailable)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(context)
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_payment_event(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    provider_account_id: Uuid,
    event_type: &str,
    failure_code: Option<String>,
    provider_payload: &Value,
    now: OffsetDateTime,
) -> Result<OrderId, ApplicationError> {
    let captured = matches!(event_type, "payment.authorized" | "payment.captured");
    let failed = matches!(event_type, "payment.failed" | "payment.cancelled");
    if !captured && !failed {
        return Err(corrupt_webhook_payload());
    }
    let (order_status, payment_status, shopper_id, currency): (String, String, Uuid, String) =
        sqlx::query_as(
            "SELECT status::text, payment_status::text, shopper_id, currency::text \
             FROM commerce.orders WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;

    let provider_bound = sqlx::query(
        "UPDATE commerce.orders \
         SET updated_at = $4 \
         WHERE store_id = $1 AND id = $2 \
           AND payment_provider_account_id = $3",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(provider_account_id)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if provider_bound.rows_affected() != 1 {
        return Err(provider_unavailable());
    }

    if captured {
        if order_status != "pending" {
            if payment_status == "failed" || order_status == "cancelled" {
                return Err(payment_event_out_of_order());
            }
            return Ok(order_id);
        }
        let applied = update_order_payment_status(
            transaction,
            store_id,
            order_id,
            &["pending"],
            "paid",
            now,
        )
        .await?;
        if !applied {
            if payment_status == "failed" {
                return Err(payment_event_out_of_order());
            }
            return Ok(order_id);
        }
        let mut event_amount: i64 = sqlx::query_scalar(
            "SELECT total_amount_minor FROM commerce.orders WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?;
        if let Some(snapshot) = StripeCheckoutSnapshot::from_payload(provider_payload)? {
            event_amount =
                apply_stripe_checkout_snapshot(transaction, store_id, order_id, &snapshot, now)
                    .await?;
        }
        confirm_paid_order(transaction, store_id, order_id, &order_status, now).await?;
        let items = load_order_analytics_items(
            transaction,
            store_id.as_uuid(),
            order_id.as_uuid(),
        )
        .await?;
        let checkout_attribution = load_checkout_attribution(
            transaction,
            store_id.as_uuid(),
            order_id.as_uuid(),
        )
        .await?;
        let (contact_email, contact_phone, storefront_origin): (
            Option<String>,
            Option<String>,
            String,
        ) = sqlx::query_as(
            "SELECT order_row.contact_email::text, order_row.contact_phone, \
                    channel.storefront_origin \
             FROM commerce.orders AS order_row \
             JOIN commerce.store_sales_channels AS channel \
               ON channel.store_id = order_row.store_id \
              AND channel.id = order_row.sales_channel_id \
             WHERE order_row.store_id = $1 AND order_row.id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?;
        let occurred_at = provider_event_time(provider_payload, now);
        let mut properties = json!({
            "_source": "server",
            "order_id": order_id.as_uuid(),
            "value_minor": event_amount,
            "currency": currency,
            "items": items,
        });
        merge_attribution(&mut properties, &checkout_attribution);
        merge_order_identity(
            &mut properties,
            contact_email.as_deref(),
            contact_phone.as_deref(),
            Some(&storefront_origin),
        );
        append_event(
            transaction,
            AnalyticsEventToAppend {
                store_id: store_id.as_uuid(),
                shopper_id,
                event_id: order_id.as_uuid(),
                event_name: "purchase".into(),
                event_source: "server",
                properties,
                occurred_at,
                received_at: now,
            },
        )
        .await?;
    } else if failed {
        let failure_code = failure_code.unwrap_or_else(|| "provider_failure".into());
        let applied = sqlx::query(
            "UPDATE commerce.orders SET payment_status = 'failed', payment_failure_code = $3, \
                    updated_at = $4 \
             WHERE store_id = $1 AND id = $2 AND payment_status = 'pending'",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(&failure_code)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?
        .rows_affected()
            == 1;
        if !applied {
            return Ok(order_id);
        }
        cancel_pending_order(transaction, store_id, order_id, &order_status, now).await?;
    }
    Ok(order_id)
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
        // Newer Stripe API versions report the collected shipping address
        // under `collected_information.shipping_details` rather than the
        // session object's own top-level `shipping_details`.
        let shipping_details = object
            .get("collected_information")
            .and_then(|value| value.get("shipping_details"))
            .filter(|value| !value.is_null())
            .or_else(|| object.get("shipping_details"));
        let email = customer_details
            .and_then(|value| value.get("email"))
            .and_then(Value::as_str)
            .and_then(non_empty_text);
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
	let Some(line1) = value.get("line1").and_then(Value::as_str).and_then(non_empty_text)
	else {
		return Ok(None);
	};
	let Some(city) = value.get("city").and_then(Value::as_str).and_then(non_empty_text) else {
		return Ok(None);
	};
	let Some(country) = value.get("country").and_then(Value::as_str).and_then(non_empty_text)
	else {
		return Ok(None);
	};
	let Some(full_name) = name.and_then(non_empty_text) else {
		return Ok(None);
	};
    let country = country.to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(corrupt_webhook_payload());
	}
	Ok(Some(StripeAddressSnapshot {
		full_name,
		line1,
		line2: value
			.get("line2")
			.and_then(Value::as_str)
			.and_then(non_empty_text),
		city,
		state: value
			.get("state")
			.and_then(Value::as_str)
			.and_then(non_empty_text),
		postal_code: value
			.get("postal_code")
			.and_then(Value::as_str)
			.and_then(non_empty_text),
		country,
	}))
}

fn non_empty_text(value: &str) -> Option<String> {
	let value = value.trim();
	(!value.is_empty()).then(|| value.to_owned())
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
                payment_provider_reference_id = COALESCE(payment_provider_reference_id, $8), \
                updated_at = $9 WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(snapshot.amount_subtotal)
    .bind(snapshot.amount_discount)
    .bind(snapshot.amount_tax)
    .bind(snapshot.amount_shipping)
    .bind(snapshot.amount_total)
    .bind(snapshot.payment_intent_id.as_deref())
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
    order.cancel()?;
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
    release_order_inventory(transaction, store_id.as_uuid(), order_id.as_uuid()).await?;
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
    order.confirm()?;
    consume_order_inventory(transaction, store_id.as_uuid(), order_id.as_uuid()).await?;
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
    let tracking_capability = generate_order_tracking_capability();
    sqlx::query(
        "INSERT INTO commerce.order_tracking_tokens \
         (store_id, order_id, token_digest, expires_at, created_at) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (store_id, order_id) DO NOTHING",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(tracking_capability.digest.as_slice())
    .bind(now + ORDER_TRACKING_TOKEN_LIFETIME)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    insert_order_confirmed_event(
        transaction,
        store_id,
        order_id,
        &tracking_capability.token,
    )
    .await?;
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
    Ok(())
}

/// Applies a `refund.` provider event. `refund_id` is `Some` for a refund
/// Chaos itself initiated (via `create_refund`), whose row already exists —
/// resolved through `chaos_refund_id` metadata since an Order can have more
/// than one refund in flight at once. It is `None` for a refund created
/// outside Chaos (e.g. from the Stripe Dashboard) — that case is resolved
/// through the PaymentIntent reference and the Refund row is created here,
/// on first sight.
#[allow(clippy::too_many_arguments)]
async fn apply_refund_event(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    refund_id: Option<RefundId>,
    provider_account_id: Uuid,
    event_type: &str,
    provider_reference_id: String,
    failure_code: Option<String>,
    provider_payload: &Value,
    now: OffsetDateTime,
) -> Result<OrderId, ApplicationError> {
    let object = provider_payload
        .get("stripe_event")
        .and_then(|event| event.get("data"))
        .and_then(|data| data.get("object"))
        .ok_or_else(corrupt_webhook_payload)?;
    let amount = object
        .get("amount")
        .and_then(Value::as_i64)
        .ok_or_else(corrupt_webhook_payload)?;
    let provider_currency = object
        .get("currency")
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase)
        .ok_or_else(corrupt_webhook_payload)?;
    let provider_currency =
        CurrencyCode::parse(&provider_currency).map_err(|_| corrupt_webhook_payload())?;
    let payment_intent = object
        .get("payment_intent")
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("pi_"));
    if amount <= 0 {
        return Err(corrupt_webhook_payload());
    }
    if !matches!(
        event_type,
        "refund.pending" | "refund.succeeded" | "refund.failed"
    ) {
        return Err(corrupt_webhook_payload());
    }
    let provider_status = object
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(corrupt_webhook_payload)?;
    let target_status = match provider_status {
        "pending" | "requires_action" => "pending",
        "succeeded" => "succeeded",
        // Stripe reports Dashboard cancellation through refund.failed with
        // status=canceled. The local ledger intentionally uses its existing
        // failed state for both provider outcomes.
        "failed" | "canceled" => "failed",
        _ => return Err(corrupt_webhook_payload()),
    };

    let resolved: Option<(Uuid, Uuid)> = match refund_id {
        Some(id) => sqlx::query_as(
            "SELECT id, order_id FROM commerce.refunds \
             WHERE store_id = $1 AND id = $2 \
               AND payment_provider_account_id = $3 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(id.as_uuid())
        .bind(provider_account_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?,
        None => None,
    };

    let (refund_row_id, order_id, order_currency) = match resolved {
        Some((refund_row_id, order_id)) => {
            let order_currency: String = sqlx::query_scalar(
                "SELECT currency::text FROM commerce.orders WHERE store_id = $1 AND id = $2",
            )
            .bind(store_id.as_uuid())
            .bind(order_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(database_error)?;
            (refund_row_id, order_id, order_currency)
        }
        None => {
            let payment_intent = payment_intent.ok_or_else(corrupt_webhook_payload)?;
            let order: (Uuid, String) = sqlx::query_as(
                "SELECT id, currency::text FROM commerce.orders \
                 WHERE store_id = $1 \
                   AND payment_provider_account_id = $3 \
                   AND payment_provider_reference_id = $2 FOR UPDATE",
            )
            .bind(store_id.as_uuid())
            .bind(payment_intent)
            .bind(provider_account_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?;
            sqlx::query(
                "INSERT INTO commerce.refunds \
                 (id, store_id, order_id, currency, status, amount_minor, \
                  payment_provider_account_id, payment_provider_reference_id) \
                 VALUES ($1, $2, $3, $4, 'pending', $5, $6, $7) \
                 ON CONFLICT (store_id, payment_provider_account_id, payment_provider_reference_id) \
                 WHERE payment_provider_reference_id IS NOT NULL DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(store_id.as_uuid())
            .bind(order.0)
            .bind(&order.1)
            .bind(amount)
            .bind(provider_account_id)
            .bind(&provider_reference_id)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            let refund_row: (Uuid, Uuid) = sqlx::query_as(
                "SELECT id, order_id FROM commerce.refunds \
                 WHERE store_id = $1 AND payment_provider_account_id = $2 \
                   AND payment_provider_reference_id = $3 FOR UPDATE",
            )
            .bind(store_id.as_uuid())
            .bind(provider_account_id)
            .bind(&provider_reference_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(database_error)?;
            (refund_row.0, refund_row.1, order.1)
        }
    };
    if order_currency != provider_currency.as_str() {
        return Err(stripe_currency_mismatch());
    }

    if target_status == "succeeded" {
        let applied = sqlx::query(
            "UPDATE commerce.refunds SET status = 'succeeded', \
                    payment_provider_account_id = $3, payment_provider_reference_id = $4, \
                    failure_code = NULL, updated_at = $5 \
             WHERE store_id = $1 AND id = $2 AND status = 'pending'",
        )
        .bind(store_id.as_uuid())
        .bind(refund_row_id)
        .bind(provider_account_id)
        .bind(&provider_reference_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?
        .rows_affected()
            == 1;
        if !applied {
            return Ok(OrderId::from_uuid(order_id));
        }
        recompute_order_refund_summary(transaction, store_id, OrderId::from_uuid(order_id), now)
            .await?;
    } else if target_status == "pending" {
        sqlx::query(
            "UPDATE commerce.refunds SET status = 'pending', \
                    payment_provider_account_id = $3, payment_provider_reference_id = $4, \
                    failure_code = NULL, updated_at = $5 \
             WHERE store_id = $1 AND id = $2 AND status = 'pending'",
        )
        .bind(store_id.as_uuid())
        .bind(refund_row_id)
        .bind(provider_account_id)
        .bind(&provider_reference_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    } else {
        sqlx::query(
            "UPDATE commerce.refunds SET status = 'failed', \
                    payment_provider_account_id = $3, payment_provider_reference_id = $4, \
                    failure_code = $5, updated_at = $6 \
             WHERE store_id = $1 AND id = $2 AND status IN ('pending', 'succeeded', 'failed')",
        )
        .bind(store_id.as_uuid())
        .bind(refund_row_id)
        .bind(provider_account_id)
        .bind(&provider_reference_id)
        .bind(failure_code.unwrap_or_else(|| "provider_failure".into()))
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
        recompute_order_refund_summary(transaction, store_id, OrderId::from_uuid(order_id), now)
            .await?;
    }
    Ok(OrderId::from_uuid(order_id))
}

fn provider_event_time(payload: &Value, fallback: OffsetDateTime) -> OffsetDateTime {
    let event_created = payload
        .get("stripe_event")
        .and_then(|event| event.get("created"))
        .and_then(Value::as_i64);
    let object_created = payload
        .get("stripe_event")
        .and_then(|event| event.get("data"))
        .and_then(|data| data.get("object"))
        .and_then(|object| object.get("created"))
        .and_then(Value::as_i64);
    event_created
        .or(object_created)
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .unwrap_or(fallback)
}

fn local_refund_status(status: PaymentRefundStatus) -> &'static str {
    match status {
        PaymentRefundStatus::Pending | PaymentRefundStatus::RequiresAction => "pending",
        PaymentRefundStatus::Succeeded => "succeeded",
        PaymentRefundStatus::Failed | PaymentRefundStatus::Canceled => "failed",
    }
}

fn reconcile_refund_status(
    current_status: &str,
    observed_status: PaymentRefundStatus,
) -> &'static str {
    match current_status {
        // A later provider snapshot may confirm a pending refund or move it
        // to a terminal state.
        "pending" => local_refund_status(observed_status),
        // A failed provider snapshot may be newer than a previously received
        // succeeded event (for example, a Dashboard cancellation). A stale
        // succeeded snapshot must never reopen a failed local state.
        "succeeded" => {
            if matches!(
                observed_status,
                PaymentRefundStatus::Failed | PaymentRefundStatus::Canceled
            ) {
                "failed"
            } else {
                "succeeded"
            }
        }
        "failed" => "failed",
        _ => local_refund_status(observed_status),
    }
}

fn local_refund_failure_code(observation: &PaymentRefundObservation) -> Option<String> {
    match observation.status {
        PaymentRefundStatus::Failed => Some(
            observation
                .failure_code
                .clone()
                .unwrap_or_else(|| "provider_failure".into()),
        ),
        PaymentRefundStatus::Canceled => Some(
            observation
                .failure_code
                .clone()
                .unwrap_or_else(|| "merchant_request".into()),
        ),
        PaymentRefundStatus::Pending
        | PaymentRefundStatus::RequiresAction
        | PaymentRefundStatus::Succeeded => None,
    }
}

async fn upsert_refund_observation(
    transaction: &mut Transaction<'static, Postgres>,
    context: &RefundReconciliationContext,
    observation: &PaymentRefundObservation,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    if observation.provider_reference_id.trim().is_empty()
        || observation.provider_reference_id.chars().count() > 255
        || observation.amount_minor <= 0
    {
        return Err(stripe_invalid_response());
    }
    let order_currency: String = sqlx::query_scalar(
        "SELECT currency::text FROM commerce.orders WHERE store_id = $1 AND id = $2",
    )
    .bind(context.store_id.as_uuid())
    .bind(context.order_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if order_currency != observation.currency.as_str() {
        return Err(stripe_currency_mismatch());
    }

    let provider_row: Option<(Uuid, Uuid, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, order_id, amount_minor, status::text, failure_code FROM commerce.refunds \
         WHERE store_id = $1 AND payment_provider_account_id = $2 \
           AND payment_provider_reference_id = $3 FOR UPDATE",
    )
    .bind(context.store_id.as_uuid())
    .bind(context.provider_account_id)
    .bind(&observation.provider_reference_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let chaos_row = if let Some(chaos_refund_id) = observation.chaos_refund_id {
        sqlx::query_as::<_, (Uuid, Uuid, i64, String, Option<String>)>(
            "SELECT id, order_id, amount_minor, status::text, failure_code \
             FROM commerce.refunds \
             WHERE store_id = $1 AND id = $2 AND order_id = $3 FOR UPDATE",
        )
        .bind(context.store_id.as_uuid())
        .bind(chaos_refund_id)
        .bind(context.order_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?
    } else {
        None
    };
    if let (Some(provider_row), Some(chaos_row)) = (provider_row.as_ref(), chaos_row.as_ref())
        && provider_row.0 != chaos_row.0
    {
        return Err(ApplicationError::Conflict {
            code: "refund_reconciliation_identity_mismatch",
            message: "Stripe Refund metadata and provider reference resolve to different refunds",
        });
    }
    let existing = provider_row.or(chaos_row);
    let observed_status = local_refund_status(observation.status);
    let observed_failure_code = local_refund_failure_code(observation);
    match existing {
        Some((
            refund_id,
            order_id,
            amount_minor,
            current_status,
            current_failure_code,
        )) => {
            if order_id != context.order_id.as_uuid() || amount_minor != observation.amount_minor {
                return Err(ApplicationError::Conflict {
                    code: "refund_reconciliation_amount_mismatch",
                    message: "the Stripe Refund amount does not match the local Refund",
                });
            }
            let status = reconcile_refund_status(&current_status, observation.status);
            let failure_code = match status {
                "failed" => observed_failure_code.or(current_failure_code),
                _ => None,
            };
            sqlx::query(
                "UPDATE commerce.refunds SET status = $4::commerce.refund_status, \
                        payment_provider_reference_id = $3, failure_code = $5, updated_at = $6 \
                 WHERE store_id = $1 AND id = $2",
            )
            .bind(context.store_id.as_uuid())
            .bind(refund_id)
            .bind(&observation.provider_reference_id)
            .bind(status)
            .bind(failure_code)
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
        }
        None => {
            let status = observed_status;
            sqlx::query(
                "INSERT INTO commerce.refunds \
                 (id, store_id, order_id, currency, status, amount_minor, \
                  payment_provider_account_id, payment_provider_reference_id, failure_code, updated_at) \
                 VALUES ($1, $2, $3, $4, $5::commerce.refund_status, $6, $7, $8, $9, $10)",
            )
            .bind(Uuid::now_v7())
            .bind(context.store_id.as_uuid())
            .bind(context.order_id.as_uuid())
            .bind(observation.currency.as_str())
            .bind(status)
            .bind(observation.amount_minor)
            .bind(context.provider_account_id)
            .bind(&observation.provider_reference_id)
            .bind(observed_failure_code)
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
        }
    }
    Ok(())
}

impl PostgresStripeRepository {
    pub(crate) async fn apply_refund_reconciliation(
        &self,
        context: &RefundReconciliationContext,
        observations: &[PaymentRefundObservation],
        now: OffsetDateTime,
    ) -> Result<(i64, Vec<RefundDetail>), ApplicationError> {
        let mut transaction = self.begin_context(None, context.store_id.as_uuid()).await?;
        let order_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce.orders \
             WHERE store_id = $1 AND id = $2 \
               AND payment_provider_account_id = $3 \
               AND payment_provider_reference_id = $4)",
        )
        .bind(context.store_id.as_uuid())
        .bind(context.order_id.as_uuid())
        .bind(context.provider_account_id)
        .bind(&context.payment_provider_reference)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !order_exists {
            return Err(provider_unavailable());
        }
        let order_currency: String = sqlx::query_scalar(
            "SELECT currency::text FROM commerce.orders WHERE store_id = $1 AND id = $2",
        )
        .bind(context.store_id.as_uuid())
        .bind(context.order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "SELECT id FROM commerce.orders WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(context.store_id.as_uuid())
        .bind(context.order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        for observation in observations {
            upsert_refund_observation(&mut transaction, context, observation, now).await?;
        }
        recompute_order_refund_summary(
            &mut transaction,
            context.store_id,
            context.order_id,
            now,
        )
        .await?;
        let refunded_amount_minor: i64 = sqlx::query_scalar(
            "SELECT refunded_amount_minor FROM commerce.orders WHERE store_id = $1 AND id = $2",
        )
        .bind(context.store_id.as_uuid())
        .bind(context.order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let rows: Vec<RefundDetailRow> = sqlx::query_as(
            "SELECT id, status::text, amount_minor, payment_provider_reference_id, \
                    failure_code, created_at, updated_at \
             FROM commerce.refunds WHERE store_id = $1 AND order_id = $2 \
             ORDER BY created_at, id",
        )
        .bind(context.store_id.as_uuid())
        .bind(context.order_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let refunds = rows
            .into_iter()
            .map(
                |(
                    id,
                    status,
                    amount_minor,
                    provider_reference_id,
                    failure_code,
                    created_at,
                    updated_at,
                )| {
                    Ok(RefundDetail {
                        id: RefundId::from_uuid(id),
                        order_id: context.order_id,
                        amount_minor,
                        currency: CurrencyCode::parse(&order_currency)?,
                        status: RefundStatus::parse(&status).ok_or_else(corrupt_payment_state)?,
                        provider_reference_id,
                        failure_code,
                        created_at,
                        updated_at,
                    })
                },
            )
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        transaction.commit().await.map_err(database_error)?;
        Ok((refunded_amount_minor, refunds))
    }
}

#[cfg(test)]
mod refund_reconciliation_tests {
    use super::{PaymentRefundStatus, non_empty_text, reconcile_refund_status};

    #[test]
    fn stale_provider_snapshots_do_not_reopen_terminal_refunds() {
        assert_eq!(
            reconcile_refund_status("failed", PaymentRefundStatus::Succeeded),
            "failed"
        );
        assert_eq!(
            reconcile_refund_status("succeeded", PaymentRefundStatus::Failed),
            "failed"
        );
        assert_eq!(
            reconcile_refund_status("succeeded", PaymentRefundStatus::Canceled),
            "failed"
        );
    }

    #[test]
    fn blank_optional_stripe_text_is_stored_as_absent() {
        assert_eq!(non_empty_text("  "), None);
        assert_eq!(non_empty_text(" Suite 100 "), Some("Suite 100".into()));
    }
}
