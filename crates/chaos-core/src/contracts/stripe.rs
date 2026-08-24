use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    payments::{PaymentAttemptStatus, RefundId, RefundStatus},
    sales::OrderId,
    stripe::{PaymentSecretReference, StripeAccount, StripeAccountId},
};
use secrecy::SecretString;
use serde_json::Value;
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

pub struct StripeWebhookEvent {
    pub stripe_account_id: Uuid,
    pub stripe_event_id: String,
    pub event_type: String,
    pub object_reference: String,
    pub order_id: Option<Uuid>,
    /// Present only for `refund.*` events Chaos itself created — resolves
    /// which Refund row this event confirms when an Order has more than one
    /// in flight. Absent for a refund created outside Chaos (e.g. the
    /// Stripe Dashboard), which is instead resolved via the PaymentIntent.
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
    /// The Order this command acts on — always present, and what
    /// `chaos_order_id` in Stripe metadata carries.
    pub aggregate_id: Uuid,
    /// The specific Refund row this command creates — only set for
    /// `refund.create_requested`. An Order can have more than one refund in
    /// flight at once, so the webhook confirming a refund needs this (via
    /// `chaos_refund_id` metadata) to know which row to update; order_id
    /// alone cannot disambiguate.
    pub refund_id: Option<Uuid>,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub idempotency_key: String,
    pub credential_secret_reference: PaymentSecretReference,
    pub stripe_payment_reference: Option<String>,
    /// Required when creating a Stripe Checkout Session; absent for Stripe
    /// commands that do not create a Checkout Session.
    pub checkout_details: Option<PaymentCheckoutDetails>,
    pub return_url: Option<String>,
    /// Written into the Stripe object's metadata purely so an operator
    /// reading the object in the Stripe Dashboard sees full Chaos context
    /// without switching back to the admin tooling. Not read back from any
    /// webhook — order_id/refund_id alone drive webhook processing.
    pub order_context: OrderMetadataContext,
}

#[derive(Clone)]
pub struct OrderMetadataContext {
    pub store_id: Uuid,
    pub shopper_id: Uuid,
    pub sales_channel_id: Uuid,
    pub order_number: String,
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
pub trait StripeWebhookSignatureVerifier: Send + Sync {
    fn name(&self) -> &'static str;

    async fn verify(
        &self,
        provider_account_id: StripeAccountId,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<StripeWebhookEvent, ApplicationError>;
}

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
