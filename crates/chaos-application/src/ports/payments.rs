use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    merchant::StoreId,
    payments::{PaymentAttemptId, PaymentAttemptStatus, RefundId, RefundStatus},
    sales::OrderId,
};
use secrecy::SecretString;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApplicationError, merchant::MerchantActor};

use super::{IdempotencyRequest, ShopperActor};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaymentProviderReadinessStatus {
    Unchecked,
    Ready,
    ActionRequired,
}

impl PaymentProviderReadinessStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchecked => "unchecked",
            Self::Ready => "ready",
            Self::ActionRequired => "action_required",
        }
    }
}

pub struct PaymentProviderReadiness {
    pub ready: bool,
    pub blocker_codes: Vec<String>,
    pub configuration: Value,
    pub checked_at: OffsetDateTime,
}

pub struct PaymentProviderAccountConfiguration {
    pub credential_secret_reference: chaos_domain::payments::PaymentSecretReference,
    pub webhook_secret_reference: chaos_domain::payments::PaymentSecretReference,
    pub readiness: Option<PaymentProviderReadiness>,
}

pub struct PaymentProviderAccountDetail {
    pub account: chaos_domain::payments::PaymentProviderAccount,
    pub credentials_configured: bool,
    pub readiness_status: PaymentProviderReadinessStatus,
    pub readiness_checked_at: Option<OffsetDateTime>,
    pub readiness_blocker_codes: Vec<String>,
    pub credential_rotation_expires_at: Option<OffsetDateTime>,
    pub webhook_rotation_expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct PaymentProviderAccountPage {
    pub items: Vec<PaymentProviderAccountDetail>,
    pub has_more: bool,
}

pub struct PaymentAttemptDetail {
    pub id: PaymentAttemptId,
    pub order_id: OrderId,
    pub provider: String,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub status: PaymentAttemptStatus,
    pub provider_reference: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct RefundDetail {
    pub id: RefundId,
    pub payment_attempt_id: PaymentAttemptId,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub status: RefundStatus,
    pub provider_reference: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct VerifiedWebhookEvent {
    pub provider: String,
    pub provider_event_id: String,
    pub event_type: String,
    pub external_account_reference: String,
    pub object_reference: String,
    pub failure_code: Option<String>,
    pub payload: Value,
    pub verified_at: OffsetDateTime,
}

pub struct QueueJob {
    pub id: Uuid,
    pub merchant_account_id: Uuid,
    pub store_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub attempts: u32,
}

pub struct ProviderCommand {
    pub event_type: String,
    pub aggregate_id: Uuid,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub idempotency_key: String,
    pub external_account_reference: String,
    pub credential_secret_reference: chaos_domain::payments::PaymentSecretReference,
    pub payment_provider_reference: Option<String>,
}

pub struct ProviderCommandResult {
    pub provider_reference: String,
}

pub struct ProviderClientActionCommand {
    pub provider_reference: String,
    pub external_account_reference: String,
    pub credential_secret_reference: chaos_domain::payments::PaymentSecretReference,
}

pub struct PaymentClientAction {
    pub provider: String,
    pub kind: &'static str,
    pub public_key: SecretString,
    pub client_token: SecretString,
    pub account_reference: String,
}

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn execute(
        &self,
        command: ProviderCommand,
    ) -> Result<ProviderCommandResult, ApplicationError>;

    async fn client_action(
        &self,
        command: ProviderClientActionCommand,
    ) -> Result<PaymentClientAction, ApplicationError>;
}

#[async_trait]
pub trait PaymentProviderOnboarding: Send + Sync {
    fn name(&self) -> &'static str;

    async fn check_readiness(
        &self,
        external_account_reference: &str,
        credential_secret_reference: &chaos_domain::payments::PaymentSecretReference,
        checked_at: OffsetDateTime,
    ) -> Result<PaymentProviderReadiness, ApplicationError>;
}

#[async_trait]
pub trait PaymentWebhookVerifier: Send + Sync {
    fn name(&self) -> &'static str;

    async fn verify(
        &self,
        provider: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedWebhookEvent, ApplicationError>;
}

#[async_trait]
pub trait PaymentWebhookConfigurationRepository: Send + Sync {
    async fn webhook_secret_references(
        &self,
        provider: &str,
        external_account_reference: &str,
    ) -> Result<Vec<chaos_domain::payments::PaymentSecretReference>, ApplicationError>;
}

#[async_trait]
pub trait PaymentRepository: Send + Sync {
    async fn create_attempt(
        &self,
        actor: &ShopperActor,
        order_id: OrderId,
        provider: &str,
        idempotency: &IdempotencyRequest,
    ) -> Result<PaymentAttemptDetail, ApplicationError>;

    async fn get_attempt(
        &self,
        actor: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<Option<PaymentAttemptDetail>, ApplicationError>;

    async fn create_refund(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        attempt_id: PaymentAttemptId,
        amount_minor: i64,
        idempotency: &IdempotencyRequest,
    ) -> Result<RefundDetail, ApplicationError>;

    async fn ingest_webhook(&self, event: &VerifiedWebhookEvent) -> Result<bool, ApplicationError>;

    async fn process_webhook_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn prepare_provider_command(
        &self,
        job: &QueueJob,
    ) -> Result<ProviderCommand, ApplicationError>;

    async fn record_provider_result(
        &self,
        job: &QueueJob,
        result: &ProviderCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn client_action_command(
        &self,
        actor: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<Option<(String, ProviderClientActionCommand)>, ApplicationError>;
}

#[async_trait]
pub trait PaymentSecretResolver: Send + Sync {
    async fn resolve(
        &self,
        reference: &chaos_domain::payments::PaymentSecretReference,
    ) -> Result<SecretString, ApplicationError>;
}

#[async_trait]
pub trait PaymentProviderAccountRepository: Send + Sync {
    async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<PaymentProviderAccountPage, ApplicationError>;

    async fn get(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        id: chaos_domain::payments::PaymentProviderAccountId,
    ) -> Result<Option<PaymentProviderAccountDetail>, ApplicationError>;

    async fn create(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        account: &chaos_domain::payments::PaymentProviderAccount,
        configuration: &PaymentProviderAccountConfiguration,
        idempotency: &IdempotencyRequest,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError>;

    async fn update(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        account: &chaos_domain::payments::PaymentProviderAccount,
        configuration: &PaymentProviderAccountConfiguration,
        idempotency: &IdempotencyRequest,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError>;
}

#[async_trait]
pub trait IntegrationQueue: Send + Sync {
    async fn claim_outbox(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn claim_webhooks(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn finish_outbox(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn finish_webhook(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}
