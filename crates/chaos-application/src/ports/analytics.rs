use async_trait::async_trait;
use chaos_domain::{
    analytics::{
        AnalyticsPolicy, BrowserEvent, BrowserEventName, ConsentSnapshot, SessionEventContribution,
    },
    identity::UserId,
    merchant::{SalesChannelId, StoreId},
    sales::CustomerId,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApplicationError, merchant::MerchantActor};

use super::{CustomerActor, IdempotencyRequest};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyticsRateLimitDecision {
    pub allowed: bool,
    pub retry_after_seconds: u32,
}

#[async_trait]
pub trait AnalyticsCollectionRateLimiter: Send + Sync {
    async fn consume(
        &self,
        merchant_account_id: Uuid,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        anonymous_event_counts: &[(Uuid, u16)],
        event_count: u16,
    ) -> Result<AnalyticsRateLimitDecision, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAnalyticsPolicy {
    pub policy: AnalyticsPolicy,
    pub policy_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreAnalyticsPolicy {
    pub id: Option<Uuid>,
    pub store_id: StoreId,
    pub policy: AnalyticsPolicy,
    pub policy_version: String,
    pub created_by: Option<UserId>,
    pub effective_at: Option<OffsetDateTime>,
    pub created_at: Option<OffsetDateTime>,
}

use super::MachineActor;

#[async_trait]
pub trait AnalyticsEventRepository: Send + Sync {
    async fn resolve_collection_policy(
        &self,
        actor: &MachineActor,
        now: OffsetDateTime,
    ) -> Result<ResolvedAnalyticsPolicy, ApplicationError>;

    async fn append_browser_events(
        &self,
        actor: &MachineActor,
        events: &[BrowserEvent],
        collection_policy_version: &str,
        advertising_exports_enabled: bool,
        received_at: OffsetDateTime,
        retention_expires_at: OffsetDateTime,
    ) -> Result<usize, ApplicationError>;
}

#[async_trait]
pub trait AnalyticsPolicyRepository: Send + Sync {
    async fn get_store_policy(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        now: OffsetDateTime,
    ) -> Result<Option<StoreAnalyticsPolicy>, ApplicationError>;

    async fn update_store_policy(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        policy: AnalyticsPolicy,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreAnalyticsPolicy, ApplicationError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DailyBehaviorReport {
    pub sales_channel_id: SalesChannelId,
    pub report_date: time::Date,
    pub sessions: u64,
    pub events: u64,
    pub page_views: u64,
    pub product_views: u64,
    pub searches: u64,
    pub cart_line_additions: u64,
    pub checkouts_started: u64,
    pub active_engagement_milliseconds: u64,
    pub refreshed_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DailyCommerceReport {
    pub sales_channel_id: SalesChannelId,
    pub report_date: time::Date,
    pub currency: String,
    pub orders_created: u64,
    pub order_amount_minor: u64,
    pub payments_captured: u64,
    pub captured_amount_minor: u64,
    pub refunds_succeeded: u64,
    pub refunded_amount_minor: u64,
    pub fulfillments_shipped: u64,
    pub returns_completed: u64,
    pub refreshed_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DailyAttributionReport {
    pub sales_channel_id: SalesChannelId,
    pub report_date: time::Date,
    pub attribution_model: String,
    pub model_version: u16,
    pub is_direct: bool,
    pub campaign_source: Option<String>,
    pub campaign_medium: Option<String>,
    pub campaign_name: Option<String>,
    pub attributed_orders: u64,
    pub attributed_amount_minor: u64,
    pub currency: String,
    pub refreshed_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsDailyReports {
    pub behavior: Vec<DailyBehaviorReport>,
    pub commerce: Vec<DailyCommerceReport>,
    pub attribution: Vec<DailyAttributionReport>,
}

#[async_trait]
pub trait AnalyticsReportingRepository: Send + Sync {
    async fn list_daily_reports(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        from: time::Date,
        to: time::Date,
    ) -> Result<Option<AnalyticsDailyReports>, ApplicationError>;
}

pub struct AnalyticsSessionizationJob {
    pub behavior_event_id: Uuid,
    pub merchant_account_id: Uuid,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
    pub event_name: BrowserEventName,
    pub anonymous_id: Uuid,
    pub client_session_id: Uuid,
    pub occurred_at: OffsetDateTime,
    pub retention_expires_at: OffsetDateTime,
    pub active_engagement_milliseconds: Option<u32>,
    pub attempts: u32,
}

pub struct AnalyticsCommerceFactJob {
    pub id: Uuid,
    pub merchant_account_id: Uuid,
    pub store_id: StoreId,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub attempts: u32,
    pub occurred_at: OffsetDateTime,
}

pub struct AnalyticsAttributionJob {
    pub commerce_fact_id: Uuid,
    pub merchant_account_id: Uuid,
    pub store_id: StoreId,
    pub model_version: u16,
    pub attempts: u32,
}

#[async_trait]
pub trait AnalyticsAttributionQueue: Send + Sync {
    async fn claim_attribution(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<AnalyticsAttributionJob>, ApplicationError>;

    async fn attribute_order(
        &self,
        job: &AnalyticsAttributionJob,
        attributed_at: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn finish_attribution(
        &self,
        worker_id: Uuid,
        job: &AnalyticsAttributionJob,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}

#[async_trait]
pub trait AnalyticsCommerceFactQueue: Send + Sync {
    async fn claim_commerce_facts(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<AnalyticsCommerceFactJob>, ApplicationError>;

    async fn ingest_commerce_fact(
        &self,
        job: &AnalyticsCommerceFactJob,
        ingested_at: OffsetDateTime,
        attribution_model_version: u16,
        attribution_available_at: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn finish_commerce_fact(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyticsRetentionPurgeResult {
    pub behavior_events_deleted: u64,
    pub attribution_results_deleted: u64,
    pub sessions_deleted: u64,
    pub identity_links_deleted: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsIdentityLink {
    pub id: Uuid,
    pub store_id: StoreId,
    pub anonymous_id: Uuid,
    pub customer_id: CustomerId,
    pub consent_policy_version: String,
    pub collection_policy_version: String,
    pub linked_at: OffsetDateTime,
    pub retention_expires_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalyticsErasureSelector {
    Anonymous(Uuid),
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
    pub behavior_events_deleted: u64,
    pub attribution_results_deleted: u64,
    pub sessions_deleted: u64,
    pub identity_links_deleted: u64,
    pub requested_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyticsErasureBatchResult {
    pub requests_completed: u64,
    pub behavior_events_deleted: u64,
    pub attribution_results_deleted: u64,
    pub sessions_deleted: u64,
    pub identity_links_deleted: u64,
}

#[async_trait]
pub trait AnalyticsSessionizationQueue: Send + Sync {
    async fn claim_sessionization(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<AnalyticsSessionizationJob>, ApplicationError>;

    async fn finish_sessionization(
        &self,
        worker_id: Uuid,
        job: &AnalyticsSessionizationJob,
        result: Result<SessionEventContribution, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn purge_expired_data(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsRetentionPurgeResult, ApplicationError>;
}

#[async_trait]
pub trait AnalyticsPrivacyRepository: Send + Sync {
    async fn link_customer_identity(
        &self,
        actor: &CustomerActor,
        anonymous_id: Uuid,
        consent: &ConsentSnapshot,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsIdentityLink, ApplicationError>;

    async fn request_erasure(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        selector: AnalyticsErasureSelector,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsErasureRequest, ApplicationError>;

    async fn get_erasure_request(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        request_id: Uuid,
    ) -> Result<Option<AnalyticsErasureRequest>, ApplicationError>;

    async fn process_erasure_requests(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsErasureBatchResult, ApplicationError>;
}
