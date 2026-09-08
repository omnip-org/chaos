use async_trait::async_trait;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

/// Maximum number of delivery attempts for every durable integration queue.
pub const MAX_INTEGRATION_ATTEMPTS: i32 = 8;

pub const SEARCH_INDEX_QUEUE: &str = "search_index_queue";
pub const ANALYTICS_CAPI_QUEUE: &str = "analytics_capi_queue";
pub const NOTIFICATION_EMAIL_QUEUE: &str = "notification_email_queue";
pub const PROVIDER_WEBHOOKS_QUEUE: &str = "provider_webhooks_queue";

pub const PRODUCT_UPDATED_TOPIC: &str = "product.updated";
pub const CART_ITEM_ADDED_TOPIC: &str = "cart.item.added";
pub const ORDER_PAYMENT_INITIATED_TOPIC: &str = "order.payment.initiated";
pub const ORDER_PAYMENT_COMPLETED_TOPIC: &str = "order.payment.completed";
pub const ORDER_FULFILLMENT_SHIPPED_TOPIC: &str = "order.fulfillment.shipped";
pub const ORDER_FULFILLMENT_DELIVERED_TOPIC: &str = "order.fulfillment.delivered";
pub const PROVIDER_WEBHOOK_RECEIVED_TOPIC: &str = "provider.webhook.received";

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
    /// Appends the verified event to `chaos_integration.provider_webhooks` and,
    /// in the same transaction, publishes `provider.webhook.received` onto
    /// `provider_webhooks_queue` for the drain worker to apply. Returns `true`
    /// when the row was newly written, `false` when
    /// `(provider_account_id, provider_event_id)` was already audited (a
    /// provider retry) and no new job was enqueued.
    async fn record(&self, event: &VerifiedWebhookEvent) -> Result<bool, ApplicationError>;
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

/// A message claimed off a PGMQ topic-routed queue (`chaos_integration.claim_topic_queue`).
/// There is no backing row: the message body carries everything the
/// consumer needs, and completion (`finish_topic`) only ever acts on the
/// PGMQ message itself (delete, retry backoff, or archive).
pub struct TopicEventJob {
    pub msg_id: i64,
    pub payload: Value,
    pub attempts: u32,
    /// The routing key that delivered this message, read back from the PGMQ
    /// message header. Empty only for a message enqueued outside
    /// `chaos_integration.publish_topic_event`. A consumer on a fan-in queue
    /// dispatches on this rather than a field it hopes the producer set.
    pub routing_key: String,
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
