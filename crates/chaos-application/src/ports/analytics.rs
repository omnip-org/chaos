use async_trait::async_trait;
use chaos_domain::{
    analytics::{AnalyticsSettings, BrowserEvent},
    identity::UserId,
    sales::CustomerId,
    store::{SalesChannelId, StoreId},
};
use serde_json::Value;
use time::{Date, OffsetDateTime};
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
        meta_reporting_enabled: bool,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalyticsErasureSelector {
    Visitor(Uuid),
    Customer(CustomerId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalyticsErasureStatus {
    Pending,
    Completed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsErasureRequest {
    pub id: Uuid,
    pub store_id: StoreId,
    pub selector: AnalyticsErasureSelector,
    pub status: AnalyticsErasureStatus,
    pub requested_by: UserId,
    pub commerce_events_deleted: u64,
    pub visitor_links_deleted: u64,
    pub requested_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
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

    async fn request_erasure(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        selector: AnalyticsErasureSelector,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsErasureRequest, ApplicationError>;

    async fn get_erasure_request(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        request_id: Uuid,
    ) -> Result<Option<AnalyticsErasureRequest>, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetaConnection {
    pub store_id: StoreId,
    pub dataset_id: String,
    pub capi_enabled: bool,
    pub credentials_configured: bool,
    pub test_event_code_configured: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetaConnectionConfiguration {
    pub dataset_id: String,
    pub credential_secret_reference: String,
    pub test_event_code: Option<String>,
    pub capi_enabled: bool,
}

#[async_trait]
pub trait MetaConnectionRepository: Send + Sync {
    async fn get_meta_connection(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<Option<MetaConnection>, ApplicationError>;

    async fn configure_meta_connection(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        configuration: MetaConnectionConfiguration,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<MetaConnection, ApplicationError>;
}

#[derive(Clone, Debug)]
pub struct MetaDeliveryJob {
    pub id: Uuid,
    pub store_id: StoreId,
    pub commerce_event_id: Uuid,
    pub attempts: u32,
}

#[derive(Clone, Debug)]
pub struct MetaDeliveryCommand {
    pub delivery_id: Uuid,
    pub event_id: Uuid,
    pub dataset_id: String,
    pub credential_secret_reference: String,
    pub test_event_code: Option<String>,
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
pub struct MetaDeliveryReceipt {
    pub provider_reference: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetaDeliveryError {
    pub retryable: bool,
    pub message: String,
}

#[async_trait]
pub trait MetaEventDestination: Send + Sync {
    async fn send(
        &self,
        command: &MetaDeliveryCommand,
    ) -> Result<MetaDeliveryReceipt, MetaDeliveryError>;
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AnalyticsErasureBatchResult {
    pub requests_completed: u64,
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
    async fn claim_meta_deliveries(
        &self,
        limit: u16,
    ) -> Result<Vec<MetaDeliveryJob>, ApplicationError>;
    async fn load_meta_delivery(
        &self,
        job: &MetaDeliveryJob,
    ) -> Result<MetaDeliveryCommand, ApplicationError>;
    async fn finish_meta_delivery(
        &self,
        job: &MetaDeliveryJob,
        result: Result<MetaDeliveryReceipt, MetaDeliveryError>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
    async fn purge_expired(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsRetentionResult, ApplicationError>;
    async fn process_erasure_requests(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsErasureBatchResult, ApplicationError>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderMetricSnapshot {
    pub id: Uuid,
    pub store_id: StoreId,
    pub provider: String,
    pub external_account_reference: String,
    pub metric_date: Date,
    pub metric_name: String,
    pub dimensions: Value,
    pub value: String,
    pub currency: Option<String>,
    pub source_reference: Option<String>,
    pub observed_at: OffsetDateTime,
    pub raw_snapshot: Value,
}
