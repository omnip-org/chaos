// Payment outbox records, provider event application, order settlement, cancellation, and refund state.

#[allow(clippy::too_many_arguments)]
async fn insert_outbox(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    aggregate_type: &'static str,
    aggregate_id: Uuid,
    event_type: &'static str,
    provider: &str,
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
        "provider": provider,
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
            String,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            OffsetDateTime,
            OffsetDateTime,
        ),
    >(
        "SELECT attempt.id, attempt.order_id, account.provider, attempt.amount_minor, \
                attempt.currency::text, attempt.status::text, attempt.provider_reference, \
                attempt.failure_code, attempt.created_at, attempt.updated_at \
         FROM commerce.payment_attempts AS attempt \
         INNER JOIN commerce.provider_accounts AS account \
           ON account.store_id = attempt.store_id AND account.id = attempt.provider_account_id \
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
            provider: row.2,
            amount_minor: row.3,
            currency: CurrencyCode::parse(&row.4)?,
            status: PaymentAttemptStatus::parse(&row.5).ok_or_else(corrupt_payment_state)?,
            provider_reference: row.6,
            failure_code: row.7,
            created_at: row.8,
            updated_at: row.9,
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
                provider_reference, failure_code, created_at, updated_at \
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
            provider_reference: row.5,
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
    provider_reference: String,
    failure_code: Option<String>,
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
            Uuid,
        ),
    >(
        "SELECT attempt.order_id, attempt.amount_minor, attempt.currency::text, \
                attempt.status::text, attempt.provider_reference, \
                sales_order.inventory_reservation_id, sales_order.status::text, \
                sales_order.checkout_id, sales_order.shopper_id \
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
        "payment.authorized" => attempt.authorize(provider_reference)?,
        "payment.captured" => {
            if attempt.status() == PaymentAttemptStatus::Pending {
                attempt.authorize(provider_reference)?;
            } else if attempt.provider_reference() != Some(provider_reference.as_str()) {
                return Err(provider_reference_mismatch());
            }
            attempt.capture()?
        }
        "payment.failed" => attempt.fail(Some(provider_reference))?,
        "payment.cancelled" => attempt.cancel(Some(provider_reference))?,
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
         SET status = $3::commerce.payment_attempt_status, provider_reference = $4, \
             failure_code = $5, updated_at = $6 \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .bind(attempt.status().as_str())
    .bind(attempt.provider_reference())
    .bind(stored_failure)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if attempt.status() == PaymentAttemptStatus::Captured {
        confirm_paid_order(
            transaction,
            store_id,
            OrderId::from_uuid(row.0),
            CheckoutId::from_uuid(row.7),
            row.5.map(InventoryReservationId::from_uuid),
            &row.6,
            now,
        )
        .await?;
        append_event(
            transaction,
            AnalyticsEventToAppend {
                store_id: store_id.as_uuid(),
                shopper_id: row.8,
                event_id: row.0,
                event_name: "purchase".into(),
                properties: json!({
                    "_source": "server",
                    "order_id": row.0,
                    "payment_attempt_id": attempt_id.as_uuid(),
                    "value_minor": row.1,
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
            CheckoutId::from_uuid(row.7),
            row.5.map(InventoryReservationId::from_uuid),
            &row.6,
            now,
        )
        .await?;
    }
    Ok(())
}

async fn cancel_pending_order(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
    checkout_id: CheckoutId,
    reservation_id: Option<InventoryReservationId>,
    status: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let status = OrderStatus::parse(status).ok_or_else(corrupt_payment_state)?;
    if status != OrderStatus::Pending {
        return Ok(());
    }
    let mut order = Order::rehydrate(order_id, checkout_id, status);
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
    checkout_id: CheckoutId,
    reservation_id: Option<InventoryReservationId>,
    status: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let status = OrderStatus::parse(status).ok_or_else(corrupt_payment_state)?;
    if status == OrderStatus::Confirmed {
        return Ok(());
    }
    let mut order = Order::rehydrate(order_id, checkout_id, status);
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
    provider_reference: String,
    failure_code: Option<String>,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, i64, String, String, Option<String>, Uuid, Uuid)>(
        "SELECT refund.payment_attempt_id, refund.amount_minor, refund.currency::text,
                refund.status::text, refund.provider_reference, attempt.order_id, order_row.shopper_id \
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
        "refund.succeeded" => refund.succeed(provider_reference)?,
        "refund.failed" => refund.fail(provider_reference)?,
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
         SET status = $3::commerce.refund_status, provider_reference = $4, \
             failure_code = $5, updated_at = $6 \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(refund_id.as_uuid())
    .bind(refund.status().as_str())
    .bind(refund.provider_reference())
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
