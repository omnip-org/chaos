use async_trait::async_trait;
use chaos_domain::store::StoreId;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApplicationError, store::StoreActor};

use super::MachineActor;

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

#[async_trait]
pub trait AnalyticsEventRepository: Send + Sync {
    async fn append_events(
        &self,
        actor: &MachineActor,
        shopper_id: Uuid,
        events: &[AnalyticsEventInput],
        received_at: OffsetDateTime,
    ) -> Result<usize, ApplicationError>;
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

#[async_trait]
pub trait AnalyticsEventQueryRepository: Send + Sync {
    async fn list_events(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        query: AnalyticsEventQuery,
        limit: u16,
    ) -> Result<AnalyticsEventPage, ApplicationError>;
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

#[async_trait]
pub trait AnalyticsDestinationRepository: Send + Sync {
    async fn get_destination(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsDestination>, ApplicationError>;

    async fn configure_destination(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        configuration: AnalyticsDestinationConfiguration,
        now: OffsetDateTime,
    ) -> Result<AnalyticsDestination, ApplicationError>;
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

#[async_trait]
pub trait AnalyticsDeliveryRepository: Send + Sync {
    async fn schedule_deliveries(&self, limit: u16) -> Result<usize, ApplicationError>;
    async fn claim_deliveries(
        &self,
        limit: u16,
    ) -> Result<Vec<AnalyticsDeliveryJob>, ApplicationError>;
    async fn load_delivery(
        &self,
        job: &AnalyticsDeliveryJob,
    ) -> Result<AnalyticsDeliveryCommand, ApplicationError>;
    async fn finish_delivery(
        &self,
        job: &AnalyticsDeliveryJob,
        result: Result<AnalyticsDeliveryReceipt, AnalyticsDeliveryError>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}
