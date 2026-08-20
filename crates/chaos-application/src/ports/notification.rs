use async_trait::async_trait;
use chaos_domain::{
    notifications::{
        NotificationProviderAccount, NotificationProviderAccountId, NotificationSecretReference,
    },
    store::StoreId,
};
use secrecy::SecretString;
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

    async fn send(
        &self,
        credential: &NotificationSecretReference,
        message: EmailMessage,
    ) -> Result<EmailDelivery, ApplicationError>;
}

pub struct EmailDeliveryJob {
    pub id: Uuid,
    pub store_id: Uuid,
    pub recipient_email: String,
    pub template_key: String,
    pub template_version: u32,
    pub template_payload: Value,
    pub provider: String,
    pub provider_account_id: NotificationProviderAccountId,
    pub credential_secret_reference: NotificationSecretReference,
    pub sender: String,
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

    async fn record_webhook(
        &self,
        provider_account_id: NotificationProviderAccountId,
        event: &VerifiedEmailWebhook,
    ) -> Result<bool, ApplicationError>;
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
        webhook_secret_reference: &NotificationSecretReference,
        message_id: &str,
        timestamp: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedEmailWebhook, ApplicationError>;
}

pub struct NotificationProviderAccountConfiguration {
    pub credential_secret_reference: NotificationSecretReference,
    pub webhook_secret_reference: NotificationSecretReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationProviderAccountDetail {
    pub account: NotificationProviderAccount,
    pub credentials_configured: bool,
    pub webhook_configured: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct ResolvedNotificationWebhook {
    pub store_id: StoreId,
    pub provider: String,
    pub webhook_secret_reference: NotificationSecretReference,
}

#[async_trait]
pub trait NotificationProviderAccountRepository: Send + Sync {
    async fn list(
        &self,
        actor: super::AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<NotificationProviderAccountDetail>, ApplicationError>;

    async fn configure(
        &self,
        actor: super::AdminActor,
        store_id: StoreId,
        account: &NotificationProviderAccount,
        configuration: &NotificationProviderAccountConfiguration,
        idempotency: &super::IdempotencyRequest,
    ) -> Result<NotificationProviderAccountDetail, ApplicationError>;

    async fn resolve_webhook(
        &self,
        account_id: NotificationProviderAccountId,
    ) -> Result<Option<ResolvedNotificationWebhook>, ApplicationError>;
}

#[async_trait]
pub trait NotificationSecretResolver: Send + Sync {
    async fn resolve(
        &self,
        reference: &NotificationSecretReference,
    ) -> Result<SecretString, ApplicationError>;
}
