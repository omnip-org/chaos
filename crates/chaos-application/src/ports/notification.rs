use async_trait::async_trait;

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
