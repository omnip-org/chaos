// Fulfillment repository wiring and shared imports.

use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, FulfillmentAllocationInput, FulfillmentDetail, FulfillmentEventJob,
        FulfillmentEventQueue, FulfillmentRepository, IdempotencyRequest, ReturnDetail,
        ReturnLineInput, ReturnReceiptInput,
    },
};
use chaos_domain::{
    CurrencyCode, FieldViolation,
    catalog::ProductVariantId,
    fulfillment::{
        Fulfillment, FulfillmentAllocation, FulfillmentId, FulfillmentStatus, Return,
        ReturnDisposition, ReturnId, ReturnStatus, calculate_return_refund_amount,
    },
    inventory::{InventoryBalance, InventoryLocationId},
    payments::{PaymentAttempt, PaymentAttemptId, PaymentAttemptStatus, Refund, RefundId},
    pricing::Money,
    sales::{OrderId, reconcile_fulfillment_statuses},
    store::StoreId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::shared::idempotency::{self, IdempotencyScope};

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
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        for (key, value) in [
            ("app.user_id", actor.audit_user_id().as_uuid()),
            ("app.store_id", actor.store_id().as_uuid()),
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

    async fn begin_store(
        &self,
        store_id: Uuid,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}
