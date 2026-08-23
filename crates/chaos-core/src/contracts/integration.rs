use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

pub const MAX_INTEGRATION_ATTEMPTS: u32 = 8;

/// A durable outbound or inbound integration job projected from PostgreSQL.
/// The business worker owns the payload semantics; Integration owns leasing
/// and completion semantics.
pub struct QueueJob {
    pub id: Uuid,
    pub store_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub attempts: u32,
}

#[async_trait]
pub trait IntegrationQueue: Send + Sync {
    async fn claim_outbox(&self, limit: u16) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn claim_webhooks(&self, limit: u16) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn finish_outbox(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn finish_webhook(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}
