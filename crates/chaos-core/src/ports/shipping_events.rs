use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

pub struct ShippingEventJob {
    pub id: Uuid,
    pub store_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub attempts: u32,
}

#[async_trait]
pub trait ShippingEventQueue: Send + Sync {
    async fn claim_events(&self, limit: u16) -> Result<Vec<ShippingEventJob>, ApplicationError>;

    async fn process_event(
        &self,
        job: &ShippingEventJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn finish_event(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}
