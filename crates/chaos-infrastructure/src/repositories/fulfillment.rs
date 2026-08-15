use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        FulfillmentAllocationInput, FulfillmentDetail, FulfillmentEventJob, FulfillmentEventQueue,
        FulfillmentRepository, IdempotencyRequest, ReturnDetail, ReturnLineInput,
        ReturnReceiptInput,
    },
};
use chaos_domain::{
    CurrencyCode, FieldViolation,
    catalog::ProductVariantId,
    fulfillment::{
        Fulfillment, FulfillmentAllocation, FulfillmentId, FulfillmentStatus, Return,
        ReturnDisposition, ReturnId, ReturnStatus, calculate_return_refund_amount,
    },
    inventory::{InventoryLocationId, StockBalance},
    merchant::StoreId,
    payments::{PaymentAttempt, PaymentAttemptId, PaymentAttemptStatus, Refund, RefundId},
    pricing::Money,
    sales::{OrderId, reconcile_fulfillment_statuses},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

#[derive(Clone)]
pub struct PostgresFulfillmentRepository {
    pool: PgPool,
}

impl PostgresFulfillmentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: MerchantActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        for (key, value) in [
            ("app.user_id", actor.user_id().as_uuid()),
            (
                "app.merchant_account_id",
                actor.merchant_account_id().as_uuid(),
            ),
        ] {
            sqlx::query("SELECT set_config($1, $2, true)")
                .bind(key)
                .bind(value.to_string())
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
        }
        Ok(transaction)
    }

    async fn begin_account(
        &self,
        account_id: Uuid,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(account_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

#[async_trait]
impl FulfillmentRepository for PostgresFulfillmentRepository {
    async fn get_return(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        return_id: ReturnId,
    ) -> Result<Option<ReturnDetail>, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin(actor).await?;
        let detail = load_return(&mut transaction, account_id, store_id, return_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_fulfillment(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        order_id: OrderId,
        allocations: Vec<FulfillmentAllocationInput>,
        request: &IdempotencyRequest,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin(actor).await?;
        if let Some(value) = reserve(
            &mut transaction,
            account_id,
            "fulfillments.create.v1",
            request,
        )
        .await?
        {
            return replay_fulfillment(value);
        }
        lock_confirmed_order(&mut transaction, account_id, store_id, order_id).await?;
        let fulfillment = Fulfillment::create(
            order_id,
            allocations
                .into_iter()
                .map(|line| FulfillmentAllocation {
                    product_variant_id: line.product_variant_id,
                    quantity: line.quantity,
                })
                .collect(),
        )?;
        validate_fulfillment_quantities(
            &mut transaction,
            account_id,
            store_id,
            order_id,
            fulfillment.allocations(),
        )
        .await?;
        sqlx::query(
            "INSERT INTO fulfillment.fulfillments \
             (id, merchant_account_id, store_id, order_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(fulfillment.id().as_uuid())
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        for line in fulfillment.allocations() {
            sqlx::query(
                "INSERT INTO fulfillment.fulfillment_lines \
                 (merchant_account_id, store_id, fulfillment_id, product_variant_id, quantity) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(account_id)
            .bind(store_id.as_uuid())
            .bind(fulfillment.id().as_uuid())
            .bind(line.product_variant_id.as_uuid())
            .bind(i32::try_from(line.quantity).map_err(unexpected_conversion)?)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        let detail = load_fulfillment(&mut transaction, account_id, store_id, fulfillment.id())
            .await?
            .ok_or_else(|| fulfillment_not_found(fulfillment.id()))?;
        complete(
            &mut transaction,
            account_id,
            "fulfillments.create.v1",
            request,
            fulfillment_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition_fulfillment(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        fulfillment_id: FulfillmentId,
        target_status: FulfillmentStatus,
        carrier: Option<&str>,
        tracking_number: Option<&str>,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let operation = match target_status {
            FulfillmentStatus::Shipped => "fulfillments.ship.v1",
            FulfillmentStatus::Delivered => "fulfillments.deliver.v1",
            FulfillmentStatus::Cancelled => "fulfillments.cancel.v1",
            FulfillmentStatus::Pending => return Err(invalid_target()),
        };
        let mut transaction = self.begin(actor).await?;
        if let Some(value) = reserve(&mut transaction, account_id, operation, request).await? {
            return replay_fulfillment(value);
        }
        let row = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT order_id, status::text FROM fulfillment.fulfillments \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 FOR UPDATE",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(fulfillment_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| fulfillment_not_found(fulfillment_id))?;
        let allocations =
            load_allocations(&mut transaction, account_id, store_id, fulfillment_id).await?;
        let mut fulfillment = Fulfillment::rehydrate(
            fulfillment_id,
            OrderId::from_uuid(row.0),
            FulfillmentStatus::parse(&row.1).ok_or_else(corrupt_state)?,
            allocations,
        );
        match target_status {
            FulfillmentStatus::Shipped => {
                fulfillment.ship()?;
                require_tracking(carrier, tracking_number)?;
            }
            FulfillmentStatus::Delivered => fulfillment.deliver()?,
            FulfillmentStatus::Cancelled => fulfillment.cancel()?,
            FulfillmentStatus::Pending => return Err(invalid_target()),
        }
        sqlx::query(
            "UPDATE fulfillment.fulfillments SET status = $4::fulfillment.fulfillment_status, \
                    carrier = COALESCE($5, carrier), tracking_number = COALESCE($6, tracking_number), \
                    shipped_at = CASE WHEN $4 = 'shipped' THEN $7 ELSE shipped_at END, \
                    delivered_at = CASE WHEN $4 = 'delivered' THEN $7 ELSE delivered_at END, \
                    cancelled_at = CASE WHEN $4 = 'cancelled' THEN $7 ELSE cancelled_at END, \
                    updated_at = $7 \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(fulfillment_id.as_uuid())
        .bind(target_status.as_str())
        .bind(carrier)
        .bind(tracking_number)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO integration.outbox_events \
             (id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload) \
             VALUES ($1, $2, $3, 'fulfillment', $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(fulfillment_id.as_uuid())
        .bind(format!("fulfillment.{}", target_status.as_str()))
        .bind(serde_json::json!({
            "fulfillment_id": fulfillment_id.as_uuid(),
            "order_id": fulfillment.order_id().as_uuid(),
            "status": target_status.as_str(),
        }))
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if target_status == FulfillmentStatus::Shipped {
            sqlx::query(
                "INSERT INTO integration.outbox_events \
                 (id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload) \
                 VALUES ($1, $2, $3, 'fulfillment', $4, 'analytics.fulfillment.shipped', $5)",
            )
            .bind(Uuid::now_v7())
            .bind(account_id)
            .bind(store_id.as_uuid())
            .bind(fulfillment_id.as_uuid())
            .bind(serde_json::json!({ "fulfillment_id": fulfillment_id.as_uuid() }))
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        let detail = load_fulfillment(&mut transaction, account_id, store_id, fulfillment_id)
            .await?
            .ok_or_else(|| fulfillment_not_found(fulfillment_id))?;
        complete(
            &mut transaction,
            account_id,
            operation,
            request,
            fulfillment_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_return(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        order_id: OrderId,
        lines: Vec<ReturnLineInput>,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<ReturnDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin(actor).await?;
        if let Some(value) =
            reserve(&mut transaction, account_id, "returns.create.v1", request).await?
        {
            return replay_return(value);
        }
        lock_confirmed_order(&mut transaction, account_id, store_id, order_id).await?;
        validate_return_lines(&lines)?;
        validate_return_quantities(&mut transaction, account_id, store_id, order_id, &lines)
            .await?;
        let (currency, refund_lines, refund_amount_minor) =
            allocate_return_refund(&mut transaction, account_id, store_id, order_id, &lines)
                .await?;
        let returned = Return::create(order_id);
        sqlx::query(
            "INSERT INTO fulfillment.returns \
             (id, merchant_account_id, store_id, order_id, refund_amount_minor, currency, \
              requested_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(returned.id().as_uuid())
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(refund_amount_minor)
        .bind(currency.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        for line in &refund_lines {
            sqlx::query(
                "INSERT INTO fulfillment.return_lines \
                 (merchant_account_id, store_id, return_id, product_variant_id, quantity, \
                  refund_amount_minor) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(account_id)
            .bind(store_id.as_uuid())
            .bind(returned.id().as_uuid())
            .bind(line.product_variant_id.as_uuid())
            .bind(i32::try_from(line.quantity).map_err(unexpected_conversion)?)
            .bind(line.refund_amount_minor)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        let detail = load_return(&mut transaction, account_id, store_id, returned.id())
            .await?
            .ok_or_else(|| return_not_found(returned.id()))?;
        complete(
            &mut transaction,
            account_id,
            "returns.create.v1",
            request,
            return_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition_return(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        return_id: ReturnId,
        target_status: ReturnStatus,
        receipt: Vec<ReturnReceiptInput>,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<ReturnDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let operation = match target_status {
            ReturnStatus::Authorized => "returns.authorize.v1",
            ReturnStatus::Rejected => "returns.reject.v1",
            ReturnStatus::Received => "returns.receive.v1",
            ReturnStatus::Completed => "returns.complete.v1",
            ReturnStatus::Requested => return Err(invalid_target()),
        };
        let mut transaction = self.begin(actor).await?;
        if let Some(value) = reserve(&mut transaction, account_id, operation, request).await? {
            return replay_return(value);
        }
        let row = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT order_id, status::text FROM fulfillment.returns \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 FOR UPDATE",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(return_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| return_not_found(return_id))?;
        let mut returned = Return::rehydrate(
            return_id,
            OrderId::from_uuid(row.0),
            ReturnStatus::parse(&row.1).ok_or_else(corrupt_state)?,
        );
        match target_status {
            ReturnStatus::Authorized => returned.authorize()?,
            ReturnStatus::Rejected => returned.reject()?,
            ReturnStatus::Received => {
                returned.receive()?;
                receive_return(
                    &mut transaction,
                    account_id,
                    store_id,
                    return_id,
                    &receipt,
                    actor.user_id().as_uuid(),
                    now,
                )
                .await?;
            }
            ReturnStatus::Completed => {
                returned.complete()?;
                insert_return_outbox(
                    &mut transaction,
                    account_id,
                    store_id,
                    return_id,
                    returned.order_id(),
                )
                .await?;
            }
            ReturnStatus::Requested => return Err(invalid_target()),
        }
        sqlx::query(
            "UPDATE fulfillment.returns SET status = $4::fulfillment.return_status, \
                    authorized_at = CASE WHEN $4 = 'authorized' THEN $5 ELSE authorized_at END, \
                    received_at = CASE WHEN $4 = 'received' THEN $5 ELSE received_at END, \
                    completed_at = CASE WHEN $4 = 'completed' THEN $5 ELSE completed_at END, \
                    rejected_at = CASE WHEN $4 = 'rejected' THEN $5 ELSE rejected_at END, \
                    updated_at = $5 WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(return_id.as_uuid())
        .bind(target_status.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = load_return(&mut transaction, account_id, store_id, return_id)
            .await?
            .ok_or_else(|| return_not_found(return_id))?;
        complete(
            &mut transaction,
            account_id,
            operation,
            request,
            return_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
}

#[async_trait]
impl FulfillmentEventQueue for PostgresFulfillmentRepository {
    async fn claim_events(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<FulfillmentEventJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Value, i32)>(
            "SELECT id, merchant_account_id, store_id, event_type, payload, attempts \
             FROM integration.claim_fulfillment_events($1, $2, $3, $4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit.clamp(1, 100)))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            Ok(FulfillmentEventJob {
                id: row.0,
                merchant_account_id: row.1,
                store_id: row.2,
                event_type: row.3,
                payload: row.4,
                attempts: u32::try_from(row.5).map_err(unexpected_conversion)?,
            })
        })
        .collect()
    }

    async fn process_event(
        &self,
        job: &FulfillmentEventJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin_account(job.merchant_account_id).await?;
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
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (succeeded, failure) = match result {
            Ok(()) => (true, String::new()),
            Err(failure) => (false, failure),
        };
        let finished: Option<bool> =
            sqlx::query_scalar("SELECT integration.finish_outbox_event($1, $2, $3, $4, $5, $6)")
                .bind(job_id)
                .bind(worker_id)
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
        "SELECT order_id FROM fulfillment.fulfillments WHERE merchant_account_id = $1 \
         AND store_id = $2 AND id = $3",
    )
    .bind(job.merchant_account_id)
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
        "SELECT fulfillment_status::text, delivery_status::text FROM sales.orders \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 FOR UPDATE",
    )
    .bind(job.merchant_account_id)
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
              FROM sales.order_lines AS order_line \
             WHERE order_line.merchant_account_id = $1 AND order_line.store_id = $2 \
               AND order_line.order_id = $3 AND order_line.requires_shipping), \
           (SELECT COALESCE(sum(fulfillment_line.quantity), 0)::bigint \
              FROM fulfillment.fulfillment_lines AS fulfillment_line \
              INNER JOIN fulfillment.fulfillments AS fulfillment_record \
                ON fulfillment_record.merchant_account_id = fulfillment_line.merchant_account_id \
               AND fulfillment_record.store_id = fulfillment_line.store_id \
               AND fulfillment_record.id = fulfillment_line.fulfillment_id \
             WHERE fulfillment_record.merchant_account_id = $1 \
               AND fulfillment_record.store_id = $2 AND fulfillment_record.order_id = $3 \
               AND fulfillment_record.status IN ('shipped', 'delivered')), \
           (SELECT COALESCE(sum(fulfillment_line.quantity), 0)::bigint \
              FROM fulfillment.fulfillment_lines AS fulfillment_line \
              INNER JOIN fulfillment.fulfillments AS fulfillment_record \
                ON fulfillment_record.merchant_account_id = fulfillment_line.merchant_account_id \
               AND fulfillment_record.store_id = fulfillment_line.store_id \
               AND fulfillment_record.id = fulfillment_line.fulfillment_id \
             WHERE fulfillment_record.merchant_account_id = $1 \
               AND fulfillment_record.store_id = $2 AND fulfillment_record.order_id = $3 \
               AND fulfillment_record.status = 'delivered')",
    )
    .bind(job.merchant_account_id)
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
        "UPDATE sales.orders SET fulfillment_status = $4::sales.order_fulfillment_status, \
                delivery_status = $5::sales.order_delivery_status, updated_at = $6 \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
           AND (fulfillment_status IS DISTINCT FROM $4::sales.order_fulfillment_status \
             OR delivery_status IS DISTINCT FROM $5::sales.order_delivery_status)",
    )
    .bind(job.merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(order_id)
    .bind(fulfillment_status.as_str())
    .bind(delivery_status.as_str())
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO sales.order_fulfillment_transitions \
         (id, merchant_account_id, store_id, order_id, source_event_id, \
          from_fulfillment_status, to_fulfillment_status, from_delivery_status, \
          to_delivery_status, occurred_at) \
         VALUES ($1,$2,$3,$4,$5,$6::sales.order_fulfillment_status, \
                 $7::sales.order_fulfillment_status,$8::sales.order_delivery_status, \
                 $9::sales.order_delivery_status,$10) ON CONFLICT (source_event_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(job.merchant_account_id)
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
         FROM fulfillment.returns WHERE merchant_account_id = $1 AND store_id = $2 \
         AND id = $3 FOR UPDATE",
    )
    .bind(job.merchant_account_id)
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
    let attempt_row = sqlx::query_as::<_, (Uuid, i64, String, Option<String>, String)>(
        "SELECT attempt.id, attempt.amount_minor, attempt.status::text, \
                attempt.provider_reference, account.provider \
         FROM payments.payment_attempts AS attempt \
         INNER JOIN payments.provider_accounts AS account \
           ON account.merchant_account_id = attempt.merchant_account_id \
          AND account.store_id = attempt.store_id AND account.id = attempt.provider_account_id \
         WHERE attempt.merchant_account_id = $1 AND attempt.store_id = $2 \
           AND attempt.order_id = $3 AND attempt.status = 'captured' \
         ORDER BY attempt.created_at DESC, attempt.id DESC LIMIT 1 FOR UPDATE OF attempt",
    )
    .bind(job.merchant_account_id)
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
        "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM payments.refunds \
         WHERE merchant_account_id = $1 AND store_id = $2 AND payment_attempt_id = $3 \
           AND status IN ('pending', 'succeeded')",
    )
    .bind(job.merchant_account_id)
    .bind(job.store_id)
    .bind(attempt_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let refund = Refund::create(&attempt, Money::new(returned.3, currency), already_refunded)?;
    sqlx::query(
        "INSERT INTO payments.refunds \
         (id, merchant_account_id, store_id, payment_attempt_id, amount_minor, currency) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(refund.id().as_uuid())
    .bind(job.merchant_account_id)
    .bind(job.store_id)
    .bind(attempt_id.as_uuid())
    .bind(returned.3)
    .bind(currency.as_str())
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "UPDATE fulfillment.returns SET refund_id = $4, updated_at = CURRENT_TIMESTAMP \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(job.merchant_account_id)
    .bind(job.store_id)
    .bind(return_id)
    .bind(refund.id().as_uuid())
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO integration.outbox_events \
         (id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload) \
         VALUES ($1,$2,$3,'refund',$4,'refund.create_requested',$5)",
    )
    .bind(Uuid::now_v7())
    .bind(job.merchant_account_id)
    .bind(job.store_id)
    .bind(refund.id().as_uuid())
    .bind(serde_json::json!({
        "provider": attempt_row.4,
        "aggregate_id": refund.id().as_uuid(),
        "amount_minor": returned.3,
        "currency": currency.as_str(),
    }))
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

fn payload_uuid(payload: &Value, field: &'static str) -> Result<Uuid, ApplicationError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| invalid_event_payload("required UUID field is missing or invalid"))
}

fn invalid_event_payload(message: &'static str) -> ApplicationError {
    ApplicationError::Conflict {
        code: "invalid_fulfillment_event",
        message,
    }
}

fn unsupported_event(event_type: &str) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "unsupported fulfillment event type: {event_type}"
    ))
}

async fn lock_confirmed_order(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status::text FROM sales.orders WHERE merchant_account_id = $1 \
         AND store_id = $2 AND id = $3 FOR UPDATE",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    match status.as_deref() {
        Some("confirmed") => Ok(()),
        Some(_) => Err(ApplicationError::Conflict {
            code: "order_not_confirmed",
            message: "the Order is not confirmed",
        }),
        None => Err(ApplicationError::NotFound {
            resource: "order",
            id: order_id.as_uuid().to_string(),
        }),
    }
}

async fn validate_fulfillment_quantities(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
    allocations: &[FulfillmentAllocation],
) -> Result<(), ApplicationError> {
    for line in allocations {
        let quantities = sqlx::query_as::<_, (i64, i64)>(
            "SELECT order_line.quantity::bigint, COALESCE(sum(fulfillment_line.quantity) \
                    FILTER (WHERE fulfillment_record.status <> 'cancelled'), 0)::bigint \
             FROM sales.order_lines AS order_line \
             LEFT JOIN fulfillment.fulfillments AS fulfillment_record \
               ON fulfillment_record.merchant_account_id = order_line.merchant_account_id \
              AND fulfillment_record.store_id = order_line.store_id \
              AND fulfillment_record.order_id = $3 \
             LEFT JOIN fulfillment.fulfillment_lines AS fulfillment_line \
               ON fulfillment_line.merchant_account_id = fulfillment_record.merchant_account_id \
              AND fulfillment_line.store_id = fulfillment_record.store_id \
              AND fulfillment_line.fulfillment_id = fulfillment_record.id \
              AND fulfillment_line.product_variant_id = order_line.product_variant_id \
             WHERE order_line.merchant_account_id = $1 AND order_line.store_id = $2 \
               AND order_line.order_id = $3 AND order_line.product_variant_id = $4 \
               AND order_line.requires_shipping \
             GROUP BY order_line.quantity",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ApplicationError::NotFound {
            resource: "order_line",
            id: line.product_variant_id.as_uuid().to_string(),
        })?;
        if quantities.1 + i64::from(line.quantity) > quantities.0 {
            return Err(ApplicationError::Conflict {
                code: "fulfillment_quantity_exceeded",
                message: "Fulfillment quantity exceeds the unfulfilled Order quantity",
            });
        }
    }
    Ok(())
}

async fn validate_return_quantities(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
    lines: &[ReturnLineInput],
) -> Result<(), ApplicationError> {
    for line in lines {
        let delivered: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(fulfillment_line.quantity), 0)::bigint \
             FROM fulfillment.fulfillment_lines AS fulfillment_line \
             INNER JOIN fulfillment.fulfillments AS fulfillment_record \
               ON fulfillment_record.merchant_account_id = fulfillment_line.merchant_account_id \
              AND fulfillment_record.store_id = fulfillment_line.store_id \
              AND fulfillment_record.id = fulfillment_line.fulfillment_id \
             WHERE fulfillment_record.merchant_account_id = $1 AND fulfillment_record.store_id = $2 \
               AND fulfillment_record.order_id = $3 AND fulfillment_record.status = 'delivered' \
               AND fulfillment_line.product_variant_id = $4",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)?;
        let returned: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(return_line.quantity), 0)::bigint \
             FROM fulfillment.return_lines AS return_line \
             INNER JOIN fulfillment.returns AS return_record \
               ON return_record.merchant_account_id = return_line.merchant_account_id \
              AND return_record.store_id = return_line.store_id AND return_record.id = return_line.return_id \
             WHERE return_record.merchant_account_id = $1 AND return_record.store_id = $2 \
               AND return_record.order_id = $3 AND return_record.status <> 'rejected' \
               AND return_line.product_variant_id = $4",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)?;
        if returned + i64::from(line.quantity) > delivered {
            return Err(ApplicationError::Conflict {
                code: "return_quantity_exceeded",
                message: "Return quantity exceeds the delivered quantity",
            });
        }
    }
    Ok(())
}

struct ReturnRefundLine {
    product_variant_id: ProductVariantId,
    quantity: u32,
    refund_amount_minor: i64,
}

async fn allocate_return_refund(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
    lines: &[ReturnLineInput],
) -> Result<(CurrencyCode, Vec<ReturnRefundLine>, i64), ApplicationError> {
    let currency_text: String = sqlx::query_scalar(
        "SELECT currency::text FROM sales.orders \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let currency = CurrencyCode::parse(&currency_text)?;
    let mut allocated = Vec::with_capacity(lines.len());
    let mut total = 0_i64;
    for line in lines {
        let order_line = sqlx::query_as::<_, (i32, i64)>(
            "SELECT quantity, total_amount_minor FROM sales.order_lines \
             WHERE merchant_account_id = $1 AND store_id = $2 AND order_id = $3 \
               AND product_variant_id = $4",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ApplicationError::NotFound {
            resource: "order_line",
            id: line.product_variant_id.as_uuid().to_string(),
        })?;
        let reserved = sqlx::query_as::<_, (i64, i64)>(
            "SELECT COALESCE(sum(return_line.quantity), 0)::bigint, \
                    COALESCE(sum(return_line.refund_amount_minor), 0)::bigint \
             FROM fulfillment.return_lines AS return_line \
             INNER JOIN fulfillment.returns AS return_record \
               ON return_record.merchant_account_id = return_line.merchant_account_id \
              AND return_record.store_id = return_line.store_id \
              AND return_record.id = return_line.return_id \
             WHERE return_record.merchant_account_id = $1 AND return_record.store_id = $2 \
               AND return_record.order_id = $3 AND return_record.status <> 'rejected' \
               AND return_line.product_variant_id = $4",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)?;
        let amount = calculate_return_refund_amount(
            order_line.1,
            u32::try_from(order_line.0).map_err(unexpected_conversion)?,
            line.quantity,
            u32::try_from(reserved.0).map_err(unexpected_conversion)?,
            reserved.1,
        )?;
        total = total
            .checked_add(amount)
            .ok_or_else(refund_amount_overflow)?;
        allocated.push(ReturnRefundLine {
            product_variant_id: line.product_variant_id,
            quantity: line.quantity,
            refund_amount_minor: amount,
        });
    }
    Ok((currency, allocated, total))
}

fn validate_return_lines(lines: &[ReturnLineInput]) -> Result<(), ApplicationError> {
    if lines.is_empty() {
        return Err(validation("lines", "must contain at least one line"));
    }
    for (index, line) in lines.iter().enumerate() {
        if !(1..=999).contains(&line.quantity) {
            return Err(validation("quantity", "must be between 1 and 999"));
        }
        if lines[..index]
            .iter()
            .any(|prior| prior.product_variant_id == line.product_variant_id)
        {
            return Err(validation("lines", "must not repeat a Variant"));
        }
    }
    Ok(())
}

async fn receive_return(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    return_id: ReturnId,
    receipt: &[ReturnReceiptInput],
    actor_user_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let expected: Vec<Uuid> = sqlx::query_scalar(
        "SELECT product_variant_id FROM fulfillment.return_lines \
         WHERE merchant_account_id = $1 AND store_id = $2 AND return_id = $3 \
         ORDER BY product_variant_id",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?;
    let mut actual = receipt
        .iter()
        .map(|line| line.product_variant_id.as_uuid())
        .collect::<Vec<_>>();
    actual.sort_unstable();
    if actual != expected {
        return Err(validation(
            "receipt",
            "must provide one disposition for every Return line",
        ));
    }
    for line in receipt {
        if line.disposition == ReturnDisposition::Restock && line.inventory_location_id.is_none() {
            return Err(validation(
                "inventory_location_id",
                "is required when disposition is restock",
            ));
        }
        sqlx::query(
            "UPDATE fulfillment.return_lines SET disposition = $4::fulfillment.return_disposition, \
                    inventory_location_id = $5 WHERE merchant_account_id = $1 AND store_id = $2 \
                    AND return_id = $3 AND product_variant_id = $6",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(return_id.as_uuid())
        .bind(line.disposition.as_str())
        .bind(line.inventory_location_id.map(InventoryLocationId::as_uuid))
        .bind(line.product_variant_id.as_uuid())
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
        if line.disposition == ReturnDisposition::Restock {
            restock_return_line(
                tx,
                account_id,
                store_id,
                return_id,
                line.product_variant_id,
                line.inventory_location_id.unwrap(),
                actor_user_id,
                now,
            )
            .await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn restock_return_line(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    return_id: ReturnId,
    variant_id: ProductVariantId,
    location_id: InventoryLocationId,
    actor_user_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let quantity: i64 = sqlx::query_scalar(
        "SELECT quantity::bigint FROM fulfillment.return_lines WHERE merchant_account_id = $1 \
         AND store_id = $2 AND return_id = $3 AND product_variant_id = $4",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .bind(variant_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let stock = sqlx::query_as::<_, (Uuid, i64, i64)>(
        "SELECT id, on_hand_quantity, reserved_quantity FROM inventory.stock_items \
         WHERE merchant_account_id = $1 AND store_id = $2 AND inventory_location_id = $3 \
           AND product_variant_id = $4 FOR UPDATE",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(location_id.as_uuid())
    .bind(variant_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    let (stock_id, current) = match stock {
        Some((id, on_hand, reserved)) => (id, StockBalance::new(on_hand, reserved)?),
        None => {
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM inventory.inventory_locations WHERE \
                 merchant_account_id = $1 AND store_id = $2 AND id = $3 AND status = 'active')",
            )
            .bind(account_id)
            .bind(store_id.as_uuid())
            .bind(location_id.as_uuid())
            .fetch_one(&mut **tx)
            .await
            .map_err(database_error)?;
            if !active {
                return Err(ApplicationError::NotFound {
                    resource: "inventory_location",
                    id: location_id.as_uuid().to_string(),
                });
            }
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO inventory.stock_items \
                 (id, merchant_account_id, store_id, inventory_location_id, product_variant_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(account_id)
            .bind(store_id.as_uuid())
            .bind(location_id.as_uuid())
            .bind(variant_id.as_uuid())
            .execute(&mut **tx)
            .await
            .map_err(database_error)?;
            (id, StockBalance::new(0, 0)?)
        }
    };
    let updated = current.adjust(quantity)?;
    sqlx::query(
        "UPDATE inventory.stock_items SET on_hand_quantity = $2, updated_at = $3 WHERE id = $1",
    )
    .bind(stock_id)
    .bind(updated.on_hand())
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO inventory.stock_ledger_entries \
         (id, merchant_account_id, store_id, stock_item_id, kind, on_hand_delta_quantity, \
          reserved_delta_quantity, resulting_on_hand_quantity, resulting_reserved_quantity, \
          note, actor_user_id, created_at) \
         VALUES ($1, $2, $3, $4, 'return_restock', $5, 0, $6, $7, $8, $9, $10)",
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(stock_id)
    .bind(quantity)
    .bind(updated.on_hand())
    .bind(updated.reserved())
    .bind(format!("Return {} received", return_id.as_uuid()))
    .bind(actor_user_id)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn load_allocations(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    fulfillment_id: FulfillmentId,
) -> Result<Vec<FulfillmentAllocation>, ApplicationError> {
    sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT product_variant_id, quantity FROM fulfillment.fulfillment_lines \
         WHERE merchant_account_id = $1 AND store_id = $2 AND fulfillment_id = $3 \
         ORDER BY product_variant_id",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|(variant_id, quantity)| {
        Ok(FulfillmentAllocation {
            product_variant_id: ProductVariantId::from_uuid(variant_id),
            quantity: u32::try_from(quantity).map_err(unexpected_conversion)?,
        })
    })
    .collect()
}

async fn load_fulfillment(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    fulfillment_id: FulfillmentId,
) -> Result<Option<FulfillmentDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, Option<String>, Option<String>, OffsetDateTime, OffsetDateTime)>(
        "SELECT order_id, status::text, carrier, tracking_number, created_at, updated_at \
         FROM fulfillment.fulfillments WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    let Some((order_id, status, carrier, tracking_number, created_at, updated_at)) = row else {
        return Ok(None);
    };
    let allocations = load_allocations(tx, account_id, store_id, fulfillment_id)
        .await?
        .into_iter()
        .map(|line| FulfillmentAllocationInput {
            product_variant_id: line.product_variant_id,
            quantity: line.quantity,
        })
        .collect();
    Ok(Some(FulfillmentDetail {
        id: fulfillment_id,
        order_id: OrderId::from_uuid(order_id),
        status: FulfillmentStatus::parse(&status).ok_or_else(corrupt_state)?,
        carrier,
        tracking_number,
        allocations,
        created_at,
        updated_at,
    }))
}

async fn load_return(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    return_id: ReturnId,
) -> Result<Option<ReturnDetail>, ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Option<Uuid>,
            i64,
            String,
            OffsetDateTime,
            OffsetDateTime,
        ),
    >(
        "SELECT order_id, status::text, refund_id, refund_amount_minor, currency::text, \
                created_at, updated_at FROM fulfillment.returns \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    let Some((order_id, status, refund_id, refund_amount_minor, currency, created_at, updated_at)) =
        row
    else {
        return Ok(None);
    };
    let lines = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT product_variant_id, quantity FROM fulfillment.return_lines \
         WHERE merchant_account_id = $1 AND store_id = $2 AND return_id = $3 \
         ORDER BY product_variant_id",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|(variant_id, quantity)| {
        Ok(ReturnLineInput {
            product_variant_id: ProductVariantId::from_uuid(variant_id),
            quantity: u32::try_from(quantity).map_err(unexpected_conversion)?,
        })
    })
    .collect::<Result<Vec<_>, ApplicationError>>()?;
    Ok(Some(ReturnDetail {
        id: return_id,
        order_id: OrderId::from_uuid(order_id),
        status: ReturnStatus::parse(&status).ok_or_else(corrupt_state)?,
        lines,
        refund_id: refund_id.map(RefundId::from_uuid),
        refund_amount_minor,
        currency: CurrencyCode::parse(&currency)?,
        created_at,
        updated_at,
    }))
}

async fn insert_return_outbox(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    return_id: ReturnId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO integration.outbox_events \
         (id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload) \
         VALUES ($1, $2, $3, 'return', $4, 'return.completed', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .bind(serde_json::json!({
        "return_id": return_id.as_uuid(),
        "order_id": order_id.as_uuid(),
    }))
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO integration.outbox_events \
         (id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload) \
         VALUES ($1, $2, $3, 'return', $4, 'analytics.return.completed', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .bind(serde_json::json!({ "return_id": return_id.as_uuid() }))
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct FulfillmentSnapshot {
    id: Uuid,
    order_id: Uuid,
    status: String,
    carrier: Option<String>,
    tracking_number: Option<String>,
    allocations: Vec<LineSnapshot>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct ReturnSnapshot {
    id: Uuid,
    order_id: Uuid,
    status: String,
    lines: Vec<LineSnapshot>,
    refund_id: Option<Uuid>,
    refund_amount_minor: i64,
    currency: String,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct LineSnapshot {
    product_variant_id: Uuid,
    quantity: u32,
}

fn fulfillment_snapshot(detail: &FulfillmentDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(FulfillmentSnapshot {
        id: detail.id.as_uuid(),
        order_id: detail.order_id.as_uuid(),
        status: detail.status.as_str().into(),
        carrier: detail.carrier.clone(),
        tracking_number: detail.tracking_number.clone(),
        allocations: detail
            .allocations
            .iter()
            .map(|line| LineSnapshot {
                product_variant_id: line.product_variant_id.as_uuid(),
                quantity: line.quantity,
            })
            .collect(),
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn return_snapshot(detail: &ReturnDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(ReturnSnapshot {
        id: detail.id.as_uuid(),
        order_id: detail.order_id.as_uuid(),
        status: detail.status.as_str().into(),
        lines: detail
            .lines
            .iter()
            .map(|line| LineSnapshot {
                product_variant_id: line.product_variant_id.as_uuid(),
                quantity: line.quantity,
            })
            .collect(),
        refund_id: detail.refund_id.map(RefundId::as_uuid),
        refund_amount_minor: detail.refund_amount_minor,
        currency: detail.currency.as_str().into(),
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_fulfillment(value: Value) -> Result<FulfillmentDetail, ApplicationError> {
    let snapshot: FulfillmentSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(FulfillmentDetail {
        id: FulfillmentId::from_uuid(snapshot.id),
        order_id: OrderId::from_uuid(snapshot.order_id),
        status: FulfillmentStatus::parse(&snapshot.status).ok_or_else(corrupt_state)?,
        carrier: snapshot.carrier,
        tracking_number: snapshot.tracking_number,
        allocations: snapshot
            .allocations
            .into_iter()
            .map(|line| FulfillmentAllocationInput {
                product_variant_id: ProductVariantId::from_uuid(line.product_variant_id),
                quantity: line.quantity,
            })
            .collect(),
        created_at: parse_time(&snapshot.created_at)?,
        updated_at: parse_time(&snapshot.updated_at)?,
    })
}

fn replay_return(value: Value) -> Result<ReturnDetail, ApplicationError> {
    let snapshot: ReturnSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(ReturnDetail {
        id: ReturnId::from_uuid(snapshot.id),
        order_id: OrderId::from_uuid(snapshot.order_id),
        status: ReturnStatus::parse(&snapshot.status).ok_or_else(corrupt_state)?,
        lines: snapshot
            .lines
            .into_iter()
            .map(|line| ReturnLineInput {
                product_variant_id: ProductVariantId::from_uuid(line.product_variant_id),
                quantity: line.quantity,
            })
            .collect(),
        refund_id: snapshot.refund_id.map(RefundId::from_uuid),
        refund_amount_minor: snapshot.refund_amount_minor,
        currency: CurrencyCode::parse(&snapshot.currency)?,
        created_at: parse_time(&snapshot.created_at)?,
        updated_at: parse_time(&snapshot.updated_at)?,
    })
}

async fn reserve(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Value>, ApplicationError> {
    idempotency::reserve(
        tx,
        &IdempotencyScope::MerchantAccount(account_id),
        operation,
        request,
    )
    .await
}

async fn complete(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    operation: &'static str,
    request: &IdempotencyRequest,
    snapshot: Value,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        tx,
        &IdempotencyScope::MerchantAccount(account_id),
        operation,
        request,
        200,
        snapshot,
    )
    .await
}

fn require_tracking(
    carrier: Option<&str>,
    tracking_number: Option<&str>,
) -> Result<(), ApplicationError> {
    match (carrier.map(str::trim), tracking_number.map(str::trim)) {
        (Some(carrier), Some(number)) if !carrier.is_empty() && !number.is_empty() => Ok(()),
        _ => Err(validation(
            "tracking",
            "carrier and tracking_number are required when shipping",
        )),
    }
}

fn fulfillment_not_found(id: FulfillmentId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "fulfillment",
        id: id.as_uuid().to_string(),
    }
}

fn return_not_found(id: ReturnId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "return",
        id: id.as_uuid().to_string(),
    }
}

fn invalid_target() -> ApplicationError {
    ApplicationError::Conflict {
        code: "invalid_transition_target",
        message: "the requested target status is not an operation",
    }
}

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}

fn refund_amount_overflow() -> ApplicationError {
    validation(
        "refund_amount_minor",
        "sum exceeds the supported amount range",
    )
}

fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains an unknown fulfillment state"
    ))
}

fn format_time(value: OffsetDateTime) -> Result<String, ApplicationError> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn parse_time(value: &str) -> Result<OffsetDateTime, ApplicationError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn invalid_snapshot(error: serde_json::Error) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "invalid fulfillment idempotency snapshot: {error}"
    ))
}

fn unexpected_conversion(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            ApplicationError::Unavailable {
                service: "postgresql",
                source: error.into(),
            }
        }
        _ => ApplicationError::Unexpected(error.into()),
    }
}
