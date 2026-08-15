use async_trait::async_trait;
use chaos_domain::{
    analytics::{AnalyticsPolicy, BrowserEvent, BrowserEventName, SessionEventContribution},
    identity::UserId,
    merchant::{SalesChannelId, StoreId},
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApplicationError, merchant::MerchantActor};

use super::IdempotencyRequest;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyticsRetentionPurgeResult {
    pub behavior_events_deleted: u64,
    pub sessions_deleted: u64,
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
