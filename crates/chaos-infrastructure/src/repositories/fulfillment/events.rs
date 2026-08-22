// Fulfillment event queue processing, reconciliation, and refund coordination.

#[async_trait]
impl FulfillmentEventQueue for PostgresFulfillmentRepository {
    async fn claim_events(&self, limit: u16) -> Result<Vec<FulfillmentEventJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, String, Value, i32)>(
            "SELECT id, store_id, event_type, payload, attempts \
             FROM integration.claim_fulfillment_events($1)",
        )
        .bind(i32::from(limit.clamp(1, 100)))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            Ok(FulfillmentEventJob {
                id: row.0,
                store_id: row.1,
                event_type: row.2,
                payload: row.3,
                attempts: u32::try_from(row.4).map_err(unexpected_conversion)?,
            })
        })
        .collect()
    }

    async fn process_event(
        &self,
        job: &FulfillmentEventJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin_store(job.store_id).await?;
        match job.event_type.as_str() {
            "fulfillment.shipped" | "fulfillment.delivered" | "fulfillment.cancelled" => {
                reconcile_order_fulfillment(&mut transaction, job, now).await?;
            }
            "return.completed" => {
                coordinate_return_refund(&mut transaction, job).await?;
            }
            _ => return Err(unsupported_event(&job.event_type)),
        }
        transaction.commit().await.map_err(database_error)
    }

    async fn finish_event(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (succeeded, failure) = match result {
            Ok(()) => (true, String::new()),
            Err(failure) => (false, failure),
        };
        let finished: Option<bool> =
            sqlx::query_scalar("SELECT integration.finish_event_outbox($1, $2, $3, $4, $5, $6)")
                .bind(job_id)
                .bind(i32::try_from(attempts).unwrap_or(i32::MAX))
                .bind(succeeded)
                .bind(failure)
                .bind(8_i32)
                .bind(now)
                .fetch_one(&self.pool)
                .await
                .map_err(database_error)?;
        if finished == Some(true) {
            Ok(())
        } else {
            Err(ApplicationError::NotFound {
                resource: "fulfillment_event_job",
                id: job_id.to_string(),
            })
        }
    }
}

async fn reconcile_order_fulfillment(
    tx: &mut Transaction<'static, Postgres>,
    job: &FulfillmentEventJob,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let fulfillment_id = payload_uuid(&job.payload, "fulfillment_id")?;
    let payload_order_id = payload_uuid(&job.payload, "order_id")?;
    let store_id = StoreId::from_uuid(job.store_id);
    let order_id: Uuid = sqlx::query_scalar(
        "SELECT order_id FROM commerce.fulfillments WHERE store_id = $1 AND id = $2",
    )
    .bind(job.store_id)
    .bind(fulfillment_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ApplicationError::NotFound {
        resource: "fulfillment",
        id: fulfillment_id.to_string(),
    })?;
    if order_id != payload_order_id {
        return Err(invalid_event_payload(
            "order_id does not match the Fulfillment",
        ));
    }
    let current = sqlx::query_as::<_, (String, String)>(
        "SELECT fulfillment_status::text, delivery_status::text FROM commerce.orders \
         WHERE store_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(job.store_id)
    .bind(order_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ApplicationError::NotFound {
        resource: "order",
        id: order_id.to_string(),
    })?;
    let quantities = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT \
           (SELECT COALESCE(sum(order_line.quantity), 0)::bigint \
              FROM commerce.order_lines AS order_line \
             WHERE order_line.store_id = $1 \
               AND order_line.order_id = $2 AND order_line.requires_shipping), \
           (SELECT COALESCE(sum(fulfillment_line.quantity), 0)::bigint \
              FROM commerce.fulfillment_lines AS fulfillment_line \
              INNER JOIN commerce.fulfillments AS fulfillment_record \
                ON fulfillment_record.store_id = fulfillment_line.store_id \
               AND fulfillment_record.id = fulfillment_line.fulfillment_id \
             WHERE fulfillment_record.store_id = $1 AND fulfillment_record.order_id = $2 \
               AND fulfillment_record.status IN ('shipped', 'delivered')), \
           (SELECT COALESCE(sum(fulfillment_line.quantity), 0)::bigint \
              FROM commerce.fulfillment_lines AS fulfillment_line \
              INNER JOIN commerce.fulfillments AS fulfillment_record \
                ON fulfillment_record.store_id = fulfillment_line.store_id \
               AND fulfillment_record.id = fulfillment_line.fulfillment_id \
             WHERE fulfillment_record.store_id = $1 AND fulfillment_record.order_id = $2 \
               AND fulfillment_record.status = 'delivered')",
    )
    .bind(job.store_id)
    .bind(order_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let (fulfillment_status, delivery_status) = reconcile_fulfillment_statuses(
        u64::try_from(quantities.0).map_err(unexpected_conversion)?,
        u64::try_from(quantities.1).map_err(unexpected_conversion)?,
        u64::try_from(quantities.2).map_err(unexpected_conversion)?,
    )?;
    sqlx::query(
        "UPDATE commerce.orders SET fulfillment_status = $3::commerce.order_fulfillment_status, \
                delivery_status = $4::commerce.order_delivery_status, updated_at = $5 \
         WHERE store_id = $1 AND id = $2 \
           AND (fulfillment_status IS DISTINCT FROM $3::commerce.order_fulfillment_status \
             OR delivery_status IS DISTINCT FROM $4::commerce.order_delivery_status)",
    )
    .bind(store_id.as_uuid())
    .bind(order_id)
    .bind(fulfillment_status.as_str())
    .bind(delivery_status.as_str())
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO commerce.order_fulfillment_transitions \
         (id, store_id, order_id, source_event_id, \
          from_fulfillment_status, to_fulfillment_status, from_delivery_status, \
          to_delivery_status, occurred_at) \
         VALUES ($1,$2,$3,$4,$5::commerce.order_fulfillment_status, \
                 $6::commerce.order_fulfillment_status,$7::commerce.order_delivery_status, \
                 $8::commerce.order_delivery_status,$9) ON CONFLICT (source_event_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(job.store_id)
    .bind(order_id)
    .bind(job.id)
    .bind(&current.0)
    .bind(fulfillment_status.as_str())
    .bind(&current.1)
    .bind(delivery_status.as_str())
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn coordinate_return_refund(
    tx: &mut Transaction<'static, Postgres>,
    job: &FulfillmentEventJob,
) -> Result<(), ApplicationError> {
    let return_id = payload_uuid(&job.payload, "return_id")?;
    let payload_order_id = payload_uuid(&job.payload, "order_id")?;
    let returned = sqlx::query_as::<_, (Uuid, String, Option<Uuid>, i64, String)>(
        "SELECT order_id, status::text, refund_id, refund_amount_minor, currency::text \
         FROM commerce.returns WHERE store_id = $1 \
         AND id = $2 FOR UPDATE",
    )
    .bind(job.store_id)
    .bind(return_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ApplicationError::NotFound {
        resource: "return",
        id: return_id.to_string(),
    })?;
    if returned.0 != payload_order_id || returned.1 != "completed" {
        return Err(invalid_event_payload(
            "Return identity or status does not match the event",
        ));
    }
    if returned.2.is_some() || returned.3 == 0 {
        return Ok(());
    }
    let attempt_row = sqlx::query_as::<_, (Uuid, i64, String, Option<String>)>(
        "SELECT attempt.id, attempt.amount_minor, attempt.status::text, \
                attempt.stripe_checkout_session_id \
         FROM commerce.payment_attempts AS attempt \
         WHERE attempt.store_id = $1 \
           AND attempt.order_id = $2 AND attempt.status = 'captured' \
         ORDER BY attempt.created_at DESC, attempt.id DESC LIMIT 1 FOR UPDATE OF attempt",
    )
    .bind(job.store_id)
    .bind(returned.0)
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ApplicationError::Conflict {
        code: "captured_payment_not_found",
        message: "a completed Return requires a captured Payment Attempt",
    })?;
    let currency = CurrencyCode::parse(&returned.4)?;
    let attempt_id = PaymentAttemptId::from_uuid(attempt_row.0);
    let attempt = PaymentAttempt::rehydrate(
        attempt_id,
        OrderId::from_uuid(returned.0),
        Money::new(attempt_row.1, currency),
        PaymentAttemptStatus::parse(&attempt_row.2).ok_or_else(corrupt_state)?,
        attempt_row.3,
    );
    let already_refunded: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM commerce.refunds \
         WHERE store_id = $1 AND payment_attempt_id = $2 \
           AND status IN ('pending', 'succeeded')",
    )
    .bind(job.store_id)
    .bind(attempt_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let refund = Refund::create(&attempt, Money::new(returned.3, currency), already_refunded)?;
    sqlx::query(
        "INSERT INTO commerce.refunds \
         (id, store_id, payment_attempt_id, amount_minor, currency) \
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(refund.id().as_uuid())
    .bind(job.store_id)
    .bind(attempt_id.as_uuid())
    .bind(returned.3)
    .bind(currency.as_str())
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "UPDATE commerce.returns SET refund_id = $3, updated_at = CURRENT_TIMESTAMP \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(job.store_id)
    .bind(return_id)
    .bind(refund.id().as_uuid())
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO integration.event_outbox \
         (id, store_id, aggregate_type, aggregate_id, event_type, payload) \
         VALUES ($1,$2,'refund',$3,'refund.create_requested',$4)",
    )
    .bind(Uuid::now_v7())
    .bind(job.store_id)
    .bind(refund.id().as_uuid())
    .bind(serde_json::json!({
        "aggregate_id": refund.id().as_uuid(),
        "amount_minor": returned.3,
        "currency": currency.as_str(),
    }))
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}
