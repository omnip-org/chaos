use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    payments::{PaymentAttemptStatus, RefundId, RefundStatus},
    sales::OrderId,
    stripe::{PaymentSecretReference, StripeAccount, StripeAccountId},
};
use secrecy::SecretString;
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

pub struct StripeAccountConfiguration {
    pub credential_secret_reference: PaymentSecretReference,
    pub webhook_secret_reference: PaymentSecretReference,
}

pub struct StripeAccountDetail {
    pub account: StripeAccount,
    pub credentials_configured: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct StripeAccountPage {
    pub items: Vec<StripeAccountDetail>,
    pub has_more: bool,
}

pub struct PaymentAttemptDetail {
    pub order_id: OrderId,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub status: PaymentAttemptStatus,
    pub provider_reference_id: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct RefundDetail {
    pub id: RefundId,
    pub order_id: OrderId,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub status: RefundStatus,
    pub provider_reference_id: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// A current Refund snapshot read from the payment provider. Webhooks are
/// notifications, while reconciliation uses this provider-owned snapshot as
/// the source of truth for a Refund's amount and terminal state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentRefundObservation {
    pub provider_reference_id: String,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub status: PaymentRefundStatus,
    pub failure_code: Option<String>,
    /// Present for Refunds created by Chaos. Dashboard-created Refunds leave
    /// this empty and are matched by their Stripe Refund ID instead.
    pub chaos_refund_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaymentRefundStatus {
    Pending,
    RequiresAction,
    Succeeded,
    Failed,
    Canceled,
}

impl PaymentRefundStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::RequiresAction => "requires_action",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

pub struct PaymentWebhookEvent {
    pub provider_account_id: Uuid,
    pub provider_event_id: String,
    pub provider_event_type: String,
    pub normalized_event_type: Option<String>,
    pub object_reference: Option<String>,
    pub order_id: Option<Uuid>,
    /// Present only for `refund.*` events Chaos itself created — resolves
    /// which Refund row this event confirms when an Order has more than one
    /// in flight. Absent for a refund created outside Chaos (e.g. the
    /// provider dashboard), which is instead resolved via the provider
    /// payment reference.
    pub refund_id: Option<Uuid>,
    pub failure_code: Option<String>,
    pub payload: Value,
    pub verified_at: OffsetDateTime,
}

pub struct StripeWebhookConfiguration {
    pub stripe_account_id: Uuid,
    pub secret_reference: PaymentSecretReference,
}

pub struct PaymentCheckoutDetails {
    /// Absent when the shopper has not supplied an email yet; Stripe
    /// Embedded Checkout collects it directly and a verified webhook
    /// backfills it onto the Order afterward.
    pub customer_email: Option<String>,
    pub customer_phone: Option<String>,
    pub shipping_address: Option<PaymentShippingAddress>,
    pub line_items: Vec<PaymentLineItem>,
    pub shipping_countries: Vec<String>,
    pub shipping_options: Vec<PaymentShippingOption>,
    pub automatic_tax: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaymentCommandKind {
    CreateCheckoutSession,
    CreateRefund,
}

pub struct PaymentLineItem {
    pub name: String,
    pub sku: Option<String>,
    /// Absolute HTTPS URL of the presentation image snapshotted onto the Order
    /// line at checkout. Stripe fetches and caches it; it must be publicly
    /// reachable.
    pub image_url: Option<String>,
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

pub struct PaymentCommand {
    pub provider_account_id: Uuid,
    pub kind: PaymentCommandKind,
    /// The Order this command acts on — always present, and what
    /// `chaos_order_id` in provider metadata carries.
    pub aggregate_id: Uuid,
    /// The specific Refund row this command creates — only set for
    /// a refund command. An Order can have more than one refund in
    /// flight at once, so the webhook confirming a refund needs this (via
    /// `chaos_refund_id` metadata) to know which row to update; order_id
    /// alone cannot disambiguate.
    pub refund_id: Option<Uuid>,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub idempotency_key: String,
    pub credential_secret_reference: String,
    pub provider_payment_reference: Option<String>,
    /// Required when creating a provider-hosted checkout; absent for commands
    /// that do not create a checkout session.
    pub checkout_details: Option<PaymentCheckoutDetails>,
    pub return_url: Option<String>,
    /// Written into the provider object's metadata when the adapter supports
    /// metadata, so an operator can see Chaos context without switching back
    /// to the admin tooling. Not read back from any webhook — order_id/refund_id
    /// alone drive webhook processing.
    pub order_context: OrderMetadataContext,
}

#[derive(Clone)]
pub struct OrderMetadataContext {
    pub store_id: Uuid,
    pub shopper_id: Uuid,
    pub channel_id: Uuid,
    pub order_number: String,
}

pub struct PaymentCommandResult {
    pub provider_object_id: String,
    pub client_action: Option<PaymentClientAction>,
}

pub struct PaymentClientAction {
    /// The client handoff for provider-hosted Embedded Checkout. The client
    /// token is the provider's checkout client secret.
    pub kind: &'static str,
    pub public_key: SecretString,
    pub client_token: SecretString,
}

/// Compatibility names for the first Payment adapter. The fields above are
/// capability-level; Stripe-specific wire mapping stays in its adapter.
pub type StripeCommand = PaymentCommand;
pub type StripeCommandResult = PaymentCommandResult;
pub type StripeWebhookEvent = PaymentWebhookEvent;

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn execute(
        &self,
        command: PaymentCommand,
    ) -> Result<PaymentCommandResult, ApplicationError>;

    /// Fetches all Refund objects for one provider payment. This is used after
    /// a charge-level refund notification and by manual reconciliation so a
    /// missed Refund webhook cannot leave the local ledger incomplete.
    async fn list_refunds(
        &self,
        credential_secret_reference: &str,
        payment_provider_reference: &str,
    ) -> Result<Vec<PaymentRefundObservation>, ApplicationError>;
}

/// Runtime registry for the Payment capability. The application selects an
/// adapter from the provider account recorded on the Order/outbox job; adding
/// another payment provider therefore does not add another branch to the
/// worker loop.
pub struct PaymentProviderRegistry {
    providers: HashMap<String, Arc<dyn PaymentProvider>>,
}

impl PaymentProviderRegistry {
    pub fn new(providers: impl IntoIterator<Item = Arc<dyn PaymentProvider>>) -> Self {
        Self {
            providers: providers
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
        }
    }

    pub fn get(&self, provider: &str) -> Option<Arc<dyn PaymentProvider>> {
        self.providers.get(provider).cloned()
    }
}

#[async_trait]
pub trait PaymentWebhookVerifier: Send + Sync {
    fn name(&self) -> &'static str;

    async fn verify(
        &self,
        provider_account_id: Uuid,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<PaymentWebhookEvent, ApplicationError>;
}

/// Runtime registry for inbound Payment webhooks. The HTTP route supplies the
/// provider name, while the registry keeps provider-specific signature logic
/// out of `PaymentService` and the router.
pub struct PaymentWebhookVerifierRegistry {
    verifiers: HashMap<String, Arc<dyn PaymentWebhookVerifier>>,
}

impl PaymentWebhookVerifierRegistry {
    pub fn new(verifiers: impl IntoIterator<Item = Arc<dyn PaymentWebhookVerifier>>) -> Self {
        Self {
            verifiers: verifiers
                .into_iter()
                .map(|verifier| (verifier.name().to_owned(), verifier))
                .collect(),
        }
    }

    pub fn get(&self, provider: &str) -> Option<Arc<dyn PaymentWebhookVerifier>> {
        self.verifiers.get(provider).cloned()
    }
}

/// Compatibility names for the existing Stripe adapter. New application code
/// should depend on `PaymentProvider` and `PaymentWebhookVerifier` instead.
pub use PaymentProvider as StripePaymentGateway;
pub use PaymentWebhookVerifier as StripeWebhookSignatureVerifier;

#[async_trait]
pub trait StripeWebhookConfigurationRepository: Send + Sync {
    async fn webhook_configuration(
        &self,
        provider_account_id: StripeAccountId,
    ) -> Result<Vec<StripeWebhookConfiguration>, ApplicationError>;
}

#[async_trait]
pub trait PaymentSecretResolver: Send + Sync {
    async fn resolve(
        &self,
        reference: &PaymentSecretReference,
    ) -> Result<SecretString, ApplicationError>;
}
