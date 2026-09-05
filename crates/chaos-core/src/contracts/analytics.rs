use async_trait::async_trait;
use chaos_domain::store::StoreId;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

/// Meta's destination config lives in `integration.provider_accounts`
/// (`capability = 'analytics', provider = 'meta'`) alongside every other
/// capability's accounts; `external_account_reference` (the Meta Dataset
/// ID) is stored inside that row's `configuration` JSONB rather than a
/// dedicated column, since `provider_accounts` doesn't have one.
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
