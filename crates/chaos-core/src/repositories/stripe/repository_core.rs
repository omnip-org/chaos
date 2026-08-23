// Payment repository core wiring, provider implementations, and shared imports.

use async_trait::async_trait;
use base64::{
    Engine,
    engine::general_purpose::URL_SAFE_NO_PAD,
};
use crate::{
    ApplicationError,
    ports::{
        AdminActor, MachineActor, PaymentAttemptDetail, PaymentCheckoutDetails,
        PaymentLineItem, StripeAccountConfiguration,
        StripeAccountDetail, StripeAccountPage,
        StripeReadiness, StripeReadinessJob, StripeReadinessQueue, StripeReadinessStatus,
        PaymentShippingAddress,
        StripeWebhookConfiguration, StripeWebhookConfigurationRepository, StripeCommand,
        StripeCommandResult, QueueJob, RefundDetail, ShopperActor,
        StripeWebhookEvent,
    },
    store::StoreActor,
};
use chaos_domain::{
    CurrencyCode,
    payments::{
        PaymentAttempt, PaymentAttemptId, PaymentAttemptStatus, Refund, RefundId, RefundStatus,
    },
    pricing::Money,
    sales::{Order, OrderId, OrderStatus},
    stripe::{PaymentSecretReference, StripeAccount, StripeAccountId},
    store::{SalesChannelId, StoreId},
};
use rand::Rng;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::{
    analytics::{AnalyticsEventToAppend, append_event},
};

const ORDER_TRACKING_TOKEN_LIFETIME: time::Duration = time::Duration::days(180);

fn generate_order_tracking_token() -> (String, [u8; 32]) {
    let mut secret = [0_u8; 32];
    rand::rng().fill_bytes(&mut secret);
    let plaintext = format!("ot_{}", URL_SAFE_NO_PAD.encode(secret));
    let digest = Sha256::digest(plaintext.as_bytes()).into();
    (plaintext, digest)
}
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
pub struct PostgresStripeRepository {
    pool: PgPool,
}

impl PostgresStripeRepository {
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

// Shared transaction helpers and provider account reconstruction.

async fn set_config(
    transaction: &mut Transaction<'static, Postgres>,
    key: &'static str,
    value: Uuid,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT set_config($1, $2, true)")
        .bind(key)
        .bind(value.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(())
}

fn unexpected_conversion(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn outbox_aggregate_id(job: &QueueJob) -> Result<Uuid, ApplicationError> {
    job.payload
        .get("aggregate_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_amount(job: &QueueJob) -> Result<i64, ApplicationError> {
    job.payload
        .get("amount_minor")
        .and_then(Value::as_i64)
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_currency(job: &QueueJob) -> Result<&str, ApplicationError> {
    job.payload
        .get("currency")
        .and_then(Value::as_str)
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_return_url(job: &QueueJob) -> Option<String> {
    job.payload
        .get("return_url")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn invalid_outbox_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("payment outbox payload is invalid"))
}

fn stripe_invalid_response() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "stripe",
        source: anyhow::anyhow!("Stripe returned an invalid object reference"),
    }
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn attempt_not_found(attempt_id: PaymentAttemptId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "payment_attempt",
        id: attempt_id.as_uuid().to_string(),
    }
}

fn refund_not_found(refund_id: RefundId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "refund",
        id: refund_id.as_uuid().to_string(),
    }
}

async fn load_stripe_account(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    id: StripeAccountId,
) -> Result<Option<StripeAccountDetail>, ApplicationError> {
    sqlx::query_as::<_, ProviderAccountRow>(
        "SELECT id, provider, display_name, enabled, \
                credential_secret_reference IS NOT NULL AND webhook_secret_reference IS NOT NULL, \
                readiness_status, readiness_checked_at, readiness_valid_until, \
                COALESCE(readiness_snapshot->'blocker_codes', '[]'::jsonb), \
                credential_rotation_expires_at, webhook_rotation_expires_at, \
                created_at, updated_at FROM commerce.payment_provider_accounts \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(stripe_account_detail)
    .transpose()
}

fn stripe_account_detail(
    row: ProviderAccountRow,
) -> Result<StripeAccountDetail, ApplicationError> {
    Ok(StripeAccountDetail {
        account: StripeAccount::rehydrate(
            StripeAccountId::from_uuid(row.0),
            row.2,
            row.3,
        )?,
        credentials_configured: row.4,
        readiness_status: match row.5.as_str() {
            "unchecked" => StripeReadinessStatus::Unchecked,
            "ready" => StripeReadinessStatus::Ready,
            "action_required" => StripeReadinessStatus::ActionRequired,
            _ => return Err(corrupt_state()),
        },
        readiness_checked_at: row.6,
        readiness_valid_until: row.7,
        readiness_blocker_codes: serde_json::from_value(row.8).map_err(|_| corrupt_state())?,
        credential_rotation_expires_at: row.9,
        webhook_rotation_expires_at: row.10,
        created_at: row.11,
        updated_at: row.12,
    })
}

fn readiness_status(readiness: &StripeReadiness) -> StripeReadinessStatus {
    if readiness.ready {
        StripeReadinessStatus::Ready
    } else {
        StripeReadinessStatus::ActionRequired
    }
}

fn map_provider_account_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error {
        let (code, message) = match database.constraint() {
            Some("payment_provider_accounts_store_provider_key") => (
                "payment_provider_already_configured",
                "the Payment Provider is already configured for this Store",
            ),
            _ => return database_error(error),
        };
        return ApplicationError::Conflict { code, message };
    }
    database_error(error)
}

fn provider_account_not_found(id: StripeAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "payment_provider_account",
        id: id.as_uuid().to_string(),
    }
}

fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Payment Provider account state"
    ))
}

fn provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_unavailable",
        message: "no enabled Payment Provider account is available",
    }
}

fn payment_order_not_pending() -> ApplicationError {
    ApplicationError::Conflict {
        code: "order_not_pending_payment",
        message: "the Order is not awaiting payment",
    }
}

fn stripe_object_mismatch() -> ApplicationError {
    ApplicationError::Conflict {
        code: "stripe_object_mismatch",
        message: "the Stripe object does not match the Payment Attempt",
    }
}

fn stripe_currency_mismatch() -> ApplicationError {
    ApplicationError::Conflict {
        code: "stripe_currency_mismatch",
        message: "the Stripe currency does not match the Payment Attempt",
    }
}

fn corrupt_payment_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains an unknown Payment state"
    ))
}

fn corrupt_webhook_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("verified webhook payload is invalid"))
}

fn queue_job_not_found() -> ApplicationError {
    ApplicationError::Conflict {
        code: "queue_lease_lost",
        message: "the queue job is no longer leased by this worker",
    }
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
