use async_trait::async_trait;
use chaos_domain::store::StoreId;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyticsRateLimitDecision {
    pub allowed: bool,
    pub retry_after_seconds: u32,
}

#[async_trait]
pub trait AnalyticsCollectionRateLimiter: Send + Sync {
    async fn consume(
        &self,
        store_id: StoreId,
        shopper_id: Uuid,
        batch_size: u16,
    ) -> Result<AnalyticsRateLimitDecision, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsEventInput {
    pub event_id: Uuid,
    pub event_name: String,
    pub occurred_at: OffsetDateTime,
    pub properties: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsEventRecord {
    pub id: Uuid,
    pub event_id: Uuid,
    pub event_name: String,
    pub shopper_id: Uuid,
    pub occurred_at: OffsetDateTime,
    pub received_at: OffsetDateTime,
    pub properties: Value,
    pub deliveries: Vec<AnalyticsEventDelivery>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsEventDelivery {
    pub provider: String,
    pub status: String,
    pub delivered_at: Option<OffsetDateTime>,
    pub provider_reference: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsEventPage {
    pub events: Vec<AnalyticsEventRecord>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyticsEventQuery {
    pub before_id: Option<Uuid>,
    pub event_name: Option<String>,
    pub source: Option<String>,
    pub delivery_status: Option<String>,
    pub shopper_id: Option<Uuid>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsDestination {
    pub id: Uuid,
    pub store_id: StoreId,
    pub provider: String,
    pub external_account_reference: String,
    pub enabled: bool,
    pub credentials_configured: bool,
    pub configuration: Value,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsDestinationConfiguration {
    pub provider: String,
    pub external_account_reference: String,
    pub credential_secret_reference: String,
    pub configuration: Value,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct AnalyticsDeliveryJob {
    pub id: Uuid,
    pub store_id: StoreId,
    pub destination_id: Uuid,
    pub analytics_event_id: Uuid,
    pub attempts: u32,
}

#[derive(Clone, Debug)]
pub struct AnalyticsDeliveryCommand {
    pub delivery_id: Uuid,
    pub provider: String,
    pub event_id: Uuid,
    pub external_account_reference: String,
    pub credential_secret_reference: String,
    pub configuration: Value,
    pub event_name: String,
    pub occurred_at: OffsetDateTime,
    pub shopper_id: Uuid,
    pub properties: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsDeliveryReceipt {
    pub provider_reference: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsDeliveryError {
    pub retryable: bool,
    pub message: String,
}

#[async_trait]
pub trait AnalyticsEventDestination: Send + Sync {
    fn provider(&self) -> &'static str;

    async fn send(
        &self,
        command: &AnalyticsDeliveryCommand,
    ) -> Result<AnalyticsDeliveryReceipt, AnalyticsDeliveryError>;
}
