// Payment repository core wiring, provider implementations, and shared imports.

use async_trait::async_trait;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, MachineActor, PaymentAttemptDetail, PaymentCheckoutDetails,
        PaymentClientAction, PaymentProvider, PaymentProviderAccountConfiguration,
        PaymentProviderAccountDetail, PaymentProviderAccountPage, PaymentProviderAccountRepository,
        PaymentProviderOnboarding, PaymentProviderReadiness, PaymentProviderReadinessJob,
        PaymentProviderReadinessQueue, PaymentProviderReadinessStatus, PaymentRepository,
        PaymentShippingAddress, PaymentWebhookConfiguration, PaymentWebhookConfigurationRepository,
        PaymentWebhookVerifier, ProviderClientActionCommand, ProviderCommand,
        ProviderCommandResult, QueueJob, RefundDetail, ShopperActor, VerifiedWebhookEvent,
    },
    store::StoreActor,
};
use chaos_domain::{
    CurrencyCode,
    inventory::InventoryReservationId,
    payments::{
        PaymentAttempt, PaymentAttemptId, PaymentAttemptStatus, PaymentProviderAccount,
        PaymentProviderAccountId, PaymentSecretReference, Refund, RefundId, RefundStatus,
    },
    pricing::Money,
    sales::{CheckoutId, Order, OrderId, OrderStatus},
    store::{SalesChannelId, StoreId},
};
use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use secrecy::SecretString;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebhookPayload {
    id: String,
    event_type: String,
    #[serde(default)]
    _account: Option<String>,
    object: String,
    #[serde(rename = "aggregate_id")]
    _aggregate_id: Uuid,
    #[serde(default)]
    failure_code: Option<String>,
}

pub struct HmacPaymentWebhookVerifier {
    secret: Vec<u8>,
}

pub struct SandboxPaymentProvider;

#[async_trait]
impl PaymentProvider for SandboxPaymentProvider {
    fn name(&self) -> &'static str {
        "testpay"
    }

    async fn execute(
        &self,
        command: ProviderCommand,
    ) -> Result<ProviderCommandResult, ApplicationError> {
        Ok(ProviderCommandResult {
            provider_reference: format!("testpay_{}", command.aggregate_id.simple()),
        })
    }

    async fn client_action(
        &self,
        command: ProviderClientActionCommand,
    ) -> Result<PaymentClientAction, ApplicationError> {
        Ok(PaymentClientAction {
            provider: "testpay".into(),
            kind: "confirm_payment",
            public_key: SecretString::from("testpay_public".to_owned()),
            client_token: SecretString::from(format!(
                "{}_client_token",
                command.provider_reference
            )),
        })
    }
}

#[async_trait]
impl PaymentProviderOnboarding for SandboxPaymentProvider {
    fn name(&self) -> &'static str {
        "testpay"
    }

    async fn check_readiness(
        &self,
        _credential_secret_reference: &PaymentSecretReference,
        checked_at: OffsetDateTime,
    ) -> Result<PaymentProviderReadiness, ApplicationError> {
        Ok(PaymentProviderReadiness {
            ready: true,
            blocker_codes: Vec::new(),
            configuration: json!({"sandbox": true}),
            checked_at,
        })
    }
}

impl HmacPaymentWebhookVerifier {
    pub fn new(secret: impl AsRef<[u8]>) -> Result<Self, anyhow::Error> {
        let secret = secret.as_ref();
        if secret.len() < 32 {
            anyhow::bail!("payment webhook secret must contain at least 32 bytes");
        }
        Ok(Self {
            secret: secret.to_vec(),
        })
    }
}

#[async_trait]
impl PaymentWebhookVerifier for HmacPaymentWebhookVerifier {
    fn name(&self) -> &'static str {
        "testpay"
    }

    async fn verify(
        &self,
        provider: &str,
        provider_account_id: Uuid,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedWebhookEvent, ApplicationError> {
        let signature = STANDARD
            .decode(signature)
            .map_err(|_| ApplicationError::Unauthorized)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        mac.update(payload);
        mac.verify_slice(&signature)
            .map_err(|_| ApplicationError::Unauthorized)?;
        if provider.trim().is_empty()
            || provider.len() > 64
            || !provider
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(ApplicationError::NotFound {
                resource: "payment_provider",
                id: provider.to_owned(),
            });
        }
        let raw: Value =
            serde_json::from_slice(payload).map_err(|_| ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "payload",
                    reason: "must be a valid payment webhook event".into(),
                }],
            })?;
        let event: WebhookPayload =
            serde_json::from_value(raw.clone()).map_err(|_| ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "payload",
                    reason: "must contain the required payment webhook fields".into(),
                }],
            })?;
        Ok(VerifiedWebhookEvent {
            provider: provider.to_owned(),
            provider_account_id,
            provider_event_id: event.id,
            event_type: event.event_type,
            object_reference: event.object,
            failure_code: event.failure_code,
            payload: raw,
            verified_at: received_at,
        })
    }
}

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
