use async_trait::async_trait;
use chaos_domain::{
    analytics::{AnalyticsSettings, BrowserEvent},
    identity::UserId,
    sales::CustomerId,
    store::{SalesChannelId, StoreId},
};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApplicationError, store::StoreActor};

use super::{CustomerActor, IdempotencyRequest, MachineActor};

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
        sales_channel_id: SalesChannelId,
        visitor_event_counts: &[(Uuid, u16)],
        batch_size: u16,
    ) -> Result<AnalyticsRateLimitDecision, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAnalyticsSettings {
    pub settings: AnalyticsSettings,
    pub revision: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreAnalyticsSettings {
    pub store_id: StoreId,
    pub revision: i32,
    pub settings: AnalyticsSettings,
    pub updated_by: Option<UserId>,
    pub updated_at: Option<OffsetDateTime>,
}

#[async_trait]
pub trait AnalyticsEventRepository: Send + Sync {
    async fn resolve_collection_settings(
        &self,
        actor: &MachineActor,
        now: OffsetDateTime,
    ) -> Result<ResolvedAnalyticsSettings, ApplicationError>;

    #[allow(clippy::too_many_arguments)]
    async fn append_browser_events(
        &self,
        actor: &MachineActor,
        events: &[BrowserEvent],
        settings_revision: i32,
        browser_collection_mode: chaos_domain::analytics::BrowserCollectionMode,
        provider_reporting_enabled: bool,
        received_at: OffsetDateTime,
        retention_expires_at: OffsetDateTime,
    ) -> Result<usize, ApplicationError>;
}

#[async_trait]
pub trait AnalyticsSettingsRepository: Send + Sync {
    async fn get_settings(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<Option<StoreAnalyticsSettings>, ApplicationError>;

    async fn update_settings(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        settings: AnalyticsSettings,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreAnalyticsSettings, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsEventRecord {
    pub id: Uuid,
    pub event_id: Uuid,
    pub event_name: String,
    pub source: String,
    pub occurred_at: OffsetDateTime,
    pub received_at: OffsetDateTime,
    pub provider_eligible: bool,
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
pub struct VisitorCustomerLink {
    pub id: Uuid,
    pub store_id: StoreId,
    pub visitor_id: Uuid,
    pub customer_id: CustomerId,
    pub consent_policy_version: String,
    pub advertising_storage_consent: bool,
    pub collection_basis: chaos_domain::analytics::BrowserCollectionBasis,
    pub settings_revision: i32,
    pub linked_at: OffsetDateTime,
    pub retention_expires_at: OffsetDateTime,
}

#[async_trait]
pub trait AnalyticsPrivacyRepository: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn link_visitor_to_customer(
        &self,
        actor: &CustomerActor,
        visitor_id: Uuid,
        consent_policy_version: &str,
        advertising_storage_consent: bool,
        collection_basis: chaos_domain::analytics::BrowserCollectionBasis,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<VisitorCustomerLink, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsConnection {
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
pub struct AnalyticsConnectionConfiguration {
    pub provider: String,
    pub external_account_reference: String,
    pub credential_secret_reference: String,
    pub configuration: Value,
    pub enabled: bool,
}

#[async_trait]
pub trait AnalyticsConnectionRepository: Send + Sync {
    async fn get_connection(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsConnection>, ApplicationError>;

    async fn configure_connection(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        configuration: AnalyticsConnectionConfiguration,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsConnection, ApplicationError>;
}

#[derive(Clone, Debug)]
pub struct AnalyticsDeliveryJob {
    pub id: Uuid,
    pub store_id: StoreId,
    pub connection_id: Uuid,
    pub commerce_event_id: Uuid,
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
    pub visitor_id: Option<Uuid>,
    pub customer_id: Option<CustomerId>,
    pub source_url: Option<String>,
    pub value_minor: Option<i64>,
    pub currency: Option<String>,
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

#[derive(Clone, Debug)]
pub struct ServerCommerceEventJob {
    pub id: Uuid,
    pub store_id: StoreId,
    pub event_type: String,
    pub aggregate_id: Uuid,
    pub payload: Value,
    pub occurred_at: OffsetDateTime,
    pub attempts: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AnalyticsRetentionResult {
    pub commerce_events_deleted: u64,
    pub visitor_links_deleted: u64,
}

#[async_trait]
pub trait AnalyticsWorkerRepository: Send + Sync {
    async fn claim_server_events(
        &self,
        limit: u16,
    ) -> Result<Vec<ServerCommerceEventJob>, ApplicationError>;
    async fn ingest_server_event(
        &self,
        job: &ServerCommerceEventJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
    async fn finish_server_event(
        &self,
        job: &ServerCommerceEventJob,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
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
    async fn purge_expired(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsRetentionResult, ApplicationError>;
}
