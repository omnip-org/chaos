use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

/// Maximum number of delivery attempts for every durable integration queue.
pub const MAX_INTEGRATION_ATTEMPTS: i32 = 8;

/// Canonical webhook envelope produced by a capability/provider verifier.
/// Verification and normalization are deliberately unaware of how the event
/// gets delivered onward.
pub struct VerifiedWebhookEvent {
    pub provider_account_id: Uuid,
    pub capability: String,
    pub provider: String,
    pub provider_event_id: String,
    /// The event name exactly as supplied by the external provider.
    pub provider_event_type: String,
    /// Chaos' optional normalized event name. `None` means the event was
    /// verified but is not understood by this application version.
    pub normalized_event_type: Option<String>,
    pub payload: Value,
    pub verified_at: OffsetDateTime,
}

#[async_trait]
pub trait WebhookInbox: Send + Sync {
    async fn record(&self, event: VerifiedWebhookEvent) -> Result<(), ApplicationError>;
}

#[async_trait]
pub trait ProviderAccountReader: Send + Sync {
    async fn resolve_webhook_secret(
        &self,
        capability: &str,
        provider: &str,
        provider_account_id: Uuid,
    ) -> Result<Option<(Uuid, String)>, ApplicationError>;
}

/// A message claimed off a PGMQ topic-routed queue (`integration.claim_topic_queue`).
/// There is no backing row: the message body carries everything the
/// consumer needs, and completion (`finish_topic`) only ever acts on the
/// PGMQ message itself (delete, retry backoff, or archive).
pub struct TopicEventJob {
    pub msg_id: i64,
    pub payload: Value,
    pub attempts: u32,
}

#[async_trait]
pub trait IntegrationQueue: Send + Sync {
    async fn claim_topic(
        &self,
        queue_name: &str,
        limit: u16,
    ) -> Result<Vec<TopicEventJob>, ApplicationError>;

    async fn finish_topic(
        &self,
        queue_name: &str,
        msg_id: i64,
        attempts: u32,
        result: Result<(), String>,
    ) -> Result<(), ApplicationError>;
}
