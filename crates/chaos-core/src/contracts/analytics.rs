use async_trait::async_trait;
use chaos_domain::store::StoreId;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsEventRecord {
    pub id: Uuid,
    pub event_id: Uuid,
    pub event_name: String,
    pub event_source: String,
    pub channel_id: Uuid,
    pub shopper_id: Uuid,
    pub session_id: Option<Uuid>,
    pub utm_source: Option<String>,
    pub utm_medium: Option<String>,
    pub utm_campaign: Option<String>,
    pub utm_term: Option<String>,
    pub utm_content: Option<String>,
    pub occurred_at: OffsetDateTime,
    pub received_at: OffsetDateTime,
    pub properties: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsEventPage {
    pub events: Vec<AnalyticsEventRecord>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyticsEventQuery {
    pub before_id: Option<Uuid>,
    pub before_received_at: Option<OffsetDateTime>,
    pub event_name: Option<String>,
    pub source: Option<String>,
    pub shopper_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub utm_source: Option<String>,
    pub utm_medium: Option<String>,
    pub utm_campaign: Option<String>,
    pub utm_term: Option<String>,
    pub utm_content: Option<String>,
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
pub struct AnalyticsDeliveryCommand {
    pub provider: String,
    pub event_id: Uuid,
    pub external_account_reference: String,
    pub credential_secret_reference: String,
    pub configuration: Value,
    pub event_name: String,
    pub event_source: String,
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
