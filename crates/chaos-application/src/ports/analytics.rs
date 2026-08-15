use async_trait::async_trait;
use chaos_domain::{
    analytics::{BrowserEvent, BrowserEventName, SessionEventContribution},
    merchant::{SalesChannelId, StoreId},
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

use super::MachineActor;

#[async_trait]
pub trait AnalyticsEventRepository: Send + Sync {
    async fn append_browser_events(
        &self,
        actor: &MachineActor,
        events: &[BrowserEvent],
        collection_policy_version: &str,
        received_at: OffsetDateTime,
        retention_expires_at: OffsetDateTime,
    ) -> Result<usize, ApplicationError>;
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
}
