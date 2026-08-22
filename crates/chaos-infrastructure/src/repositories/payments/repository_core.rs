// Payment repository core wiring, provider implementations, and shared imports.

use async_trait::async_trait;
use base64::{
    Engine,
    engine::general_purpose::URL_SAFE_NO_PAD,
};
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, MachineActor, PaymentAttemptDetail, PaymentCheckoutDetails,
        PaymentLineItem, StripeAccountConfiguration,
        StripeAccountDetail, StripeAccountPage, StripeAccountRepository,
        StripeReadiness, StripeReadinessJob, StripeReadinessQueue, StripeReadinessStatus,
        StripePaymentRepository,
        PaymentShippingAddress, PaymentShippingOption,
        StripeWebhookConfiguration, StripeWebhookConfigurationRepository, StripeCommand,
        StripeCommandResult, StripeClientActionCommand, QueueJob, RefundDetail, ShopperActor,
        StripeWebhookEvent,
    },
    store::StoreActor,
};
use chaos_domain::{
    CurrencyCode,
    inventory::InventoryReservationId,
    payments::{
        PaymentAttempt, PaymentAttemptId, PaymentAttemptStatus, StripeAccount, StripeAccountId,
        PaymentSecretReference, Refund, RefundId, RefundStatus,
    },
    pricing::Money,
    sales::{Order, OrderId, OrderStatus},
    store::{SalesChannelId, StoreId},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::{
    analytics::{AnalyticsEventToAppend, append_event},
    inventory::{ReservationClosure, close_reservation},
    shared::idempotency::{self, IdempotencyScope},
};

const CREATE_ATTEMPT_OPERATION: &str = "payment_attempts.create.v1";
const ORDER_TRACKING_KEY_LIFETIME: time::Duration = time::Duration::days(180);

fn generate_order_tracking_key() -> (String, [u8; 32]) {
    let mut secret = [0_u8; 32];
    rand::rng().fill_bytes(&mut secret);
    let plaintext = format!("otk_{}", URL_SAFE_NO_PAD.encode(secret));
    let digest = Sha256::digest(plaintext.as_bytes()).into();
    (plaintext, digest)
}
const CREATE_REFUND_OPERATION: &str = "refunds.create.v1";
const CREATE_PROVIDER_ACCOUNT_OPERATION: &str = "payment_provider_accounts.create.v1";
const UPDATE_PROVIDER_ACCOUNT_OPERATION: &str = "payment_provider_accounts.update.v1";
type ProviderAccountRow = (
    Uuid,
    String,
    String,
    bool,
    bool,
    String,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    Value,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(Clone)]
pub struct PostgresPaymentRepository {
    pool: PgPool,
}

impl PostgresPaymentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_machine(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        self.begin_context(None, actor.store_id.as_uuid()).await
    }

    async fn begin_shopper(
        &self,
        shopper: &ShopperActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.begin_machine(&shopper.machine).await?;
        set_config(
            &mut transaction,
            "app.shopper_id",
            shopper.shopper_id.as_uuid(),
        )
        .await?;
        Ok(transaction)
    }

    async fn begin_human(
        &self,
        actor: StoreActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        self.begin_context(Some(actor.user_id().as_uuid()), actor.store_id().as_uuid())
            .await
    }

    async fn begin_admin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        self.begin_context(
            Some(actor.audit_user_id().as_uuid()),
            actor.store_id().as_uuid(),
        )
        .await
    }

    async fn begin_context(
        &self,
        user_id: Option<Uuid>,
        store_id: Uuid,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        if let Some(user_id) = user_id {
            set_config(&mut transaction, "app.user_id", user_id).await?;
        }
        set_config(&mut transaction, "app.store_id", store_id).await?;
        Ok(transaction)
    }
}
