use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    payments::{PaymentAttemptId, PaymentAttemptStatus, RefundId, RefundStatus},
    sales::OrderId,
    store::StoreId,
    stripe::{PaymentSecretReference, StripeAccount, StripeAccountId},
};
use secrecy::SecretString;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StripeReadinessStatus {
    Unchecked,
    Ready,
    ActionRequired,
}

impl StripeReadinessStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchecked => "unchecked",
            Self::Ready => "ready",
            Self::ActionRequired => "action_required",
        }
    }
}

pub struct StripeReadiness {
    pub ready: bool,
    pub blocker_codes: Vec<String>,
    pub configuration: Value,
    pub checked_at: OffsetDateTime,
}

pub struct StripeAccountConfiguration {
    pub credential_secret_reference: PaymentSecretReference,
    pub webhook_secret_reference: PaymentSecretReference,
    pub readiness: Option<StripeReadiness>,
}

pub struct StripeAccountDetail {
    pub account: StripeAccount,
    pub credentials_configured: bool,
    pub readiness_status: StripeReadinessStatus,
    pub readiness_checked_at: Option<OffsetDateTime>,
    pub readiness_valid_until: Option<OffsetDateTime>,
    pub readiness_blocker_codes: Vec<String>,
    pub credential_rotation_expires_at: Option<OffsetDateTime>,
    pub webhook_rotation_expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct StripeAccountPage {
    pub items: Vec<StripeAccountDetail>,
    pub has_more: bool,
}

pub struct PaymentAttemptDetail {
    pub id: PaymentAttemptId,
    pub order_id: OrderId,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub status: PaymentAttemptStatus,
    pub stripe_checkout_session_id: Option<String>,
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
    pub stripe_refund_id: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct StripeWebhookEvent {
    pub stripe_account_id: Uuid,
    pub stripe_event_id: String,
    pub event_type: String,
    pub object_reference: String,
    pub failure_code: Option<String>,
    pub payload: Value,
    pub verified_at: OffsetDateTime,
}

pub struct StripeWebhookConfiguration {
    pub stripe_account_id: Uuid,
    pub secret_reference: PaymentSecretReference,
}

pub struct StripeReadinessJob {
    pub stripe_account_id: StripeAccountId,
    pub store_id: StoreId,
    pub credential_secret_reference: PaymentSecretReference,
    pub attempts: u32,
}

pub struct PaymentCheckoutDetails {
    pub customer_email: String,
    pub customer_phone: Option<String>,
    pub shipping_address: Option<PaymentShippingAddress>,
    pub line_items: Vec<PaymentLineItem>,
    pub shipping_countries: Vec<String>,
    pub shipping_options: Vec<PaymentShippingOption>,
    pub automatic_tax: bool,
}

pub struct PaymentLineItem {
    pub name: String,
    pub sku: Option<String>,
    pub quantity: u32,
    pub unit_amount_minor: i64,
}

pub struct PaymentShippingOption {
    pub service_id: Uuid,
    pub code: String,
    pub name: String,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub estimated_min_days: u16,
    pub estimated_max_days: u16,
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

pub struct StripeCommand {
    pub stripe_account_id: StripeAccountId,
    pub event_type: String,
    pub aggregate_id: Uuid,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub idempotency_key: String,
    pub credential_secret_reference: PaymentSecretReference,
    pub stripe_payment_reference: Option<String>,
    /// Required when creating a Stripe Checkout Session; absent for Stripe
    /// commands that do not create a Checkout Session.
    pub checkout_details: Option<PaymentCheckoutDetails>,
    pub return_url: Option<String>,
}

pub struct StripeCommandResult {
    pub stripe_object_id: String,
    pub client_action: Option<PaymentClientAction>,
}

pub struct PaymentClientAction {
    /// The client handoff for Stripe Embedded Checkout. The client token is
    /// the Checkout Session client secret.
    pub kind: &'static str,
    pub public_key: SecretString,
    pub client_token: SecretString,
}

#[async_trait]
pub trait StripePaymentGateway: Send + Sync {
    fn name(&self) -> &'static str;

    async fn execute(
        &self,
        command: StripeCommand,
    ) -> Result<StripeCommandResult, ApplicationError>;
}

#[async_trait]
pub trait StripeAccountReadiness: Send + Sync {
    fn name(&self) -> &'static str;

    async fn check_readiness(
        &self,
        credential_secret_reference: &PaymentSecretReference,
        checked_at: OffsetDateTime,
    ) -> Result<StripeReadiness, ApplicationError>;
}

#[async_trait]
pub trait StripeReadinessQueue: Send + Sync {
    async fn claim_stripe_readiness(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<StripeReadinessJob>, ApplicationError>;

    async fn finish_stripe_readiness(
        &self,
        worker_id: Uuid,
        stripe_account_id: StripeAccountId,
        result: Result<StripeReadiness, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}

#[async_trait]
pub trait StripeWebhookSignatureVerifier: Send + Sync {
    fn name(&self) -> &'static str;

    async fn verify(
        &self,
        store_id: StoreId,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<StripeWebhookEvent, ApplicationError>;
}

#[async_trait]
pub trait StripeWebhookConfigurationRepository: Send + Sync {
    async fn webhook_configurations(
        &self,
        store_id: StoreId,
    ) -> Result<Vec<StripeWebhookConfiguration>, ApplicationError>;
}

#[async_trait]
pub trait PaymentSecretResolver: Send + Sync {
    async fn resolve(
        &self,
        reference: &PaymentSecretReference,
    ) -> Result<SecretString, ApplicationError>;
}
