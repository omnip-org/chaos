use async_trait::async_trait;
use chaos_domain::analytics::BrowserEvent;
use time::OffsetDateTime;

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
