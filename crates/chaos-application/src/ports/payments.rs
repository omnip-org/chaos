use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    payments::{PaymentAttemptId, PaymentAttemptStatus, RefundId, RefundStatus},
    sales::OrderId,
    store::StoreId,
};
use secrecy::SecretString;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApplicationError, store::StoreActor};

use super::{AdminActor, IdempotencyRequest, ShopperActor};

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
    pub readiness_valid_until: Option<OffsetDateTime>,
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
    pub provider_account_id: Uuid,
    pub provider_event_id: String,
    pub event_type: String,
    pub object_reference: String,
    pub failure_code: Option<String>,
    pub payload: Value,
    pub verified_at: OffsetDateTime,
}

pub struct PaymentWebhookConfiguration {
    pub provider_account_id: Uuid,
    pub secret_reference: chaos_domain::payments::PaymentSecretReference,
}

pub struct QueueJob {
    pub id: Uuid,
    pub store_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub attempts: u32,
}

pub struct PaymentProviderReadinessJob {
    pub provider_account_id: chaos_domain::payments::PaymentProviderAccountId,
    pub store_id: StoreId,
    pub provider: String,
    pub credential_secret_reference: chaos_domain::payments::PaymentSecretReference,
    pub attempts: u32,
}

pub struct PaymentCheckoutDetails {
    pub customer_email: String,
    pub customer_phone: Option<String>,
    pub shipping_address: Option<PaymentShippingAddress>,
}

pub struct PaymentShippingAddress {
    pub name: String,
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub state: Option<String>,
    pub postal_code: Option<String>,
    pub country_code: String,
}

pub struct ProviderCommand {
    pub provider_account_id: chaos_domain::payments::PaymentProviderAccountId,
    pub event_type: String,
    pub aggregate_id: Uuid,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub idempotency_key: String,
    pub credential_secret_reference: chaos_domain::payments::PaymentSecretReference,
    pub payment_provider_reference: Option<String>,
    /// Required for `payment.create_requested` by the Stripe Checkout adapter;
    /// absent for provider commands that do not create a Checkout Session.
    pub checkout_details: Option<PaymentCheckoutDetails>,
    pub return_url: Option<String>,
}

pub struct ProviderCommandResult {
    pub provider_reference: String,
}

pub struct ProviderClientActionCommand {
    pub provider_account_id: chaos_domain::payments::PaymentProviderAccountId,
    pub provider_reference: String,
    pub credential_secret_reference: chaos_domain::payments::PaymentSecretReference,
}

pub struct PaymentClientAction {
    pub provider: String,
    /// One of `"confirm_payment"` (client_token is a PaymentIntent client
    /// secret for Stripe.js/Elements confirmation) or
    /// `"mount_embedded_checkout"` (client_token is an embedded Checkout
    /// Session client secret).
    pub kind: &'static str,
    pub public_key: SecretString,
    pub client_token: SecretString,
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
        credential_secret_reference: &chaos_domain::payments::PaymentSecretReference,
        checked_at: OffsetDateTime,
    ) -> Result<PaymentProviderReadiness, ApplicationError>;
}

#[async_trait]
pub trait PaymentProviderReadinessQueue: Send + Sync {
    async fn claim_provider_readiness(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<PaymentProviderReadinessJob>, ApplicationError>;

    async fn finish_provider_readiness(
        &self,
        worker_id: Uuid,
        provider_account_id: chaos_domain::payments::PaymentProviderAccountId,
        result: Result<PaymentProviderReadiness, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}

#[async_trait]
pub trait PaymentWebhookVerifier: Send + Sync {
    fn name(&self) -> &'static str;

    async fn verify(
        &self,
        provider: &str,
        provider_account_id: Uuid,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedWebhookEvent, ApplicationError>;
}

#[async_trait]
pub trait PaymentWebhookConfigurationRepository: Send + Sync {
    async fn webhook_configurations(
        &self,
        provider: &str,
        provider_account_id: Uuid,
    ) -> Result<Vec<PaymentWebhookConfiguration>, ApplicationError>;
}

#[async_trait]
pub trait PaymentRepository: Send + Sync {
    async fn create_attempt(
        &self,
        actor: &ShopperActor,
        order_id: OrderId,
        provider: &str,
        return_url: Option<&str>,
        idempotency: &IdempotencyRequest,
    ) -> Result<PaymentAttemptDetail, ApplicationError>;

    async fn get_attempt(
        &self,
        actor: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<Option<PaymentAttemptDetail>, ApplicationError>;

    async fn create_refund(
        &self,
        actor: AdminActor,
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
        actor: StoreActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<PaymentProviderAccountPage, ApplicationError>;

    async fn get(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        id: chaos_domain::payments::PaymentProviderAccountId,
    ) -> Result<Option<PaymentProviderAccountDetail>, ApplicationError>;

    async fn create(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        account: &chaos_domain::payments::PaymentProviderAccount,
        configuration: &PaymentProviderAccountConfiguration,
        idempotency: &IdempotencyRequest,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError>;

    async fn update(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        account: &chaos_domain::payments::PaymentProviderAccount,
        configuration: &PaymentProviderAccountConfiguration,
        idempotency: &IdempotencyRequest,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError>;
}

#[async_trait]
pub trait IntegrationQueue: Send + Sync {
    async fn claim_outbox(&self, limit: u16) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn claim_webhooks(&self, limit: u16) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn finish_outbox(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn finish_webhook(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}
