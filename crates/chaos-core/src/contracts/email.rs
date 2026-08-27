use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

/// Provider-neutral email command. Templates are rendered before a provider
/// adapter sees this value, so Resend (or a future provider) is not part of
/// order state or application use cases.
pub struct EmailMessage {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
    pub idempotency_key: String,
}

/// Store-owned configuration used to render the sender of transactional email.
/// Provider-specific credentials remain opaque references on the provider
/// account and are deliberately not part of this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmailAccountConfiguration {
    pub from_email: String,
    pub from_name: Option<String>,
}

impl EmailAccountConfiguration {
    pub fn sender(&self) -> String {
        match self.from_name.as_deref() {
            Some(from_name) => format!("{from_name} <{}>", self.from_email),
            None => self.from_email.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmailBrandConfiguration {
    pub brand_name: String,
    pub logo_url: Option<String>,
    pub primary_color: String,
    pub accent_color: String,
    pub background_color: String,
    pub surface_color: String,
    pub text_color: String,
    pub muted_text_color: String,
    pub support_email: Option<String>,
    pub support_url: Option<String>,
}

impl EmailBrandConfiguration {
    pub fn defaults(brand_name: String) -> Self {
        Self {
            brand_name,
            logo_url: None,
            primary_color: "#175CD3".into(),
            accent_color: "#0E7490".into(),
            background_color: "#F4F6F8".into(),
            surface_color: "#FFFFFF".into(),
            text_color: "#17202A".into(),
            muted_text_color: "#667085".into(),
            support_email: None,
            support_url: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmailBrandDetail {
    pub configuration: EmailBrandConfiguration,
    pub customized: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmailOrderLineItem {
    pub product_title: String,
    pub variant_title: String,
    pub sku: Option<String>,
    pub quantity: i32,
    pub unit_price_amount_minor: i64,
    pub subtotal_amount_minor: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmailProviderAccountDetail {
    pub id: Uuid,
    pub provider: String,
    pub display_name: String,
    pub enabled: bool,
    pub credentials_configured: bool,
    pub webhook_configured: bool,
    pub configuration: EmailAccountConfiguration,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmailProviderAccountPage {
    pub items: Vec<EmailProviderAccountDetail>,
    pub has_more: bool,
}

#[derive(Debug)]
pub struct EmailDelivery {
    pub provider_message_id: String,
}

#[async_trait]
pub trait EmailProvider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn send(
        &self,
        credential_secret_reference: &str,
        message: EmailMessage,
    ) -> Result<EmailDelivery, ApplicationError>;
}

pub struct VerifiedEmailWebhook {
    pub provider_event_id: String,
    pub provider_event_type: String,
    pub normalized_event_type: Option<String>,
    pub payload: Value,
    pub received_at: OffsetDateTime,
}

#[async_trait]
pub trait EmailWebhookVerifier: Send + Sync {
    fn name(&self) -> &'static str;

    async fn verify(
        &self,
        webhook_secret_reference: &str,
        message_id: &str,
        timestamp: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedEmailWebhook, ApplicationError>;
}
