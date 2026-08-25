use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;

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
