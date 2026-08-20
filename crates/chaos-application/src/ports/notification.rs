use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

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

    async fn send(&self, message: EmailMessage) -> Result<EmailDelivery, ApplicationError>;
}

pub struct EmailDeliveryJob {
    pub id: Uuid,
    pub store_id: Uuid,
    pub recipient_email: String,
    pub template_key: String,
    pub template_version: u32,
    pub template_payload: Value,
    pub provider: String,
    pub attempts: u32,
}

pub struct VerifiedEmailWebhook {
    pub provider_event_id: String,
    pub provider_message_id: String,
    pub provider_event_type: String,
    pub payload: Value,
    pub received_at: OffsetDateTime,
}

#[async_trait]
pub trait EmailDeliveryRepository: Send + Sync {
    async fn claim(&self, limit: u16) -> Result<Vec<EmailDeliveryJob>, ApplicationError>;

    async fn finish(
        &self,
        delivery_id: Uuid,
        attempts: u32,
        result: Result<EmailDelivery, EmailDeliveryFailure>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn record_webhook(&self, event: &VerifiedEmailWebhook) -> Result<bool, ApplicationError>;
}

pub struct EmailDeliveryFailure {
    pub retryable: bool,
    pub message: String,
}

#[async_trait]
pub trait EmailWebhookVerifier: Send + Sync {
    fn name(&self) -> &'static str;

    async fn verify(
        &self,
        message_id: &str,
        timestamp: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedEmailWebhook, ApplicationError>;
}
