use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

/// Maximum number of delivery attempts for every durable integration queue.
pub const MAX_INTEGRATION_ATTEMPTS: i32 = 8;

/// The terminal or retrying outcome of a verified provider webhook.
/// `Unsupported` is deliberately separate from `Failed`: an event the
/// provider sent legitimately but this version of Chaos does not understand
/// must be retained without retrying forever.
pub enum WebhookProcessingResult {
    Processed,
    Unsupported { reason: String },
    Failed { reason: String },
}

/// Canonical webhook envelope produced by a capability/provider verifier.
/// Persistence and idempotency are deliberately unaware of Stripe, Resend, or
/// a carrier's wire format.
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
    pub aggregate_type: Option<String>,
    pub aggregate_id: Option<Uuid>,
    pub verified_at: OffsetDateTime,
}

#[async_trait]
pub trait WebhookInbox: Send + Sync {
    async fn record(&self, event: VerifiedWebhookEvent) -> Result<bool, ApplicationError>;
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

/// A durable outbound or inbound integration job projected from PostgreSQL.
/// The business worker owns the payload semantics; Integration owns leasing
/// and completion semantics.
pub struct QueueJob {
    pub id: Uuid,
    pub store_id: Uuid,
    /// The queue is part of the lease contract. Business workers claim only
    /// the capability queue they own, so a new capability cannot be consumed
    /// by the payment worker by accident.
    pub queue_name: String,
    /// Set for internal events read from `integration.event_outbox`.
    pub internal_event_type: Option<String>,
    /// Set for provider webhook events read from the shared webhook inbox.
    pub provider_event_type: Option<String>,
    /// Set when the provider event has a Chaos handler. Unknown verified
    /// events intentionally leave this as `None`.
    pub normalized_event_type: Option<String>,
    pub payload: Value,
    pub attempts: u32,
    pub provider_account_id: Option<Uuid>,
    pub capability: Option<String>,
    pub provider: Option<String>,
}

/// A message claimed off a PGMQ topic-routed queue (`integration.claim_topic_queue`).
/// Unlike `QueueJob`, there is no backing row: the message body carries
/// everything the consumer needs, and completion (`finish_topic`) only ever
/// acts on the PGMQ message itself (delete, retry backoff, or archive).
pub struct TopicEventJob {
    pub msg_id: i64,
    pub payload: Value,
    pub attempts: u32,
}

#[async_trait]
pub trait IntegrationQueue: Send + Sync {
    async fn claim_outbox(
        &self,
        queue_name: &str,
        limit: u16,
    ) -> Result<Vec<QueueJob>, ApplicationError>;

    async fn claim_webhooks(
        &self,
        capability: &str,
        limit: u16,
    ) -> Result<Vec<QueueJob>, ApplicationError>;

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
        result: WebhookProcessingResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

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
