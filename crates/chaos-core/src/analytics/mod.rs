use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresCapiEventStore,
    contracts::{
        ANALYTICS_CAPI_QUEUE, AnalyticsDeliveryCommand, AnalyticsDestination,
        AnalyticsDestinationConfiguration, AnalyticsEventDestination, IntegrationQueue,
        ORDER_PAYMENT_COMPLETED_TOPIC, TopicEventFailure,
    },
    store::StoreActor,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub struct AnalyticsAdministration {
    repository: Arc<PostgresCapiEventStore>,
}

impl AnalyticsAdministration {
    pub fn new(repository: Arc<PostgresCapiEventStore>) -> Self {
        Self { repository }
    }

    pub async fn get_destination(
        &self,
        actor: StoreActor,
        store_id: chaos_domain::store::StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsDestination>, ApplicationError> {
        self.repository
            .get_destination(actor, store_id, provider)
            .await
    }

    pub async fn configure_destination(
        &self,
        actor: StoreActor,
        store_id: chaos_domain::store::StoreId,
        configuration: AnalyticsDestinationConfiguration,
        now: OffsetDateTime,
    ) -> Result<AnalyticsDestination, ApplicationError> {
        if actor.role() != chaos_domain::store::StoreRole::Owner {
            return Err(ApplicationError::Forbidden);
        }
        self.repository
            .configure_destination(actor, store_id, configuration, now)
            .await
    }
}

/// Consumes `analytics_capi_queue` (bound to `order.payment.completed`) and
/// delivers confirmed Purchases to the configured Meta CAPI destination.
/// Topic routing
/// already picked this consumer, so there's no provider-name dispatch here
/// the way a shared queue would need; a second ad-platform destination
/// would get its own queue, binding, and worker instance instead of joining
/// this one.
pub struct MetaCapiWorker {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<PostgresCapiEventStore>,
    destination: Arc<dyn AnalyticsEventDestination>,
}

impl MetaCapiWorker {
    pub fn new(
        queue: Arc<dyn IntegrationQueue>,
        repository: Arc<PostgresCapiEventStore>,
        destination: Arc<dyn AnalyticsEventDestination>,
    ) -> Self {
        Self {
            queue,
            repository,
            destination,
        }
    }

    pub async fn run_batch(&self, limit: u16) -> Result<usize, ApplicationError> {
        let jobs = self.queue.claim_topic(ANALYTICS_CAPI_QUEUE, limit).await?;
        for job in &jobs {
            let result = if is_purchase_topic(&job.routing_key) {
                self.deliver(&job.payload).await
            } else {
                tracing::info!(
                    routing_key = %job.routing_key,
                    "capi delivery filtered for unsupported event"
                );
                Ok(())
            };
            self.queue
                .finish_topic(ANALYTICS_CAPI_QUEUE, job.msg_id, job.attempts, result)
                .await?;
        }
        Ok(jobs.len())
    }

    async fn deliver(&self, payload: &Value) -> Result<(), TopicEventFailure> {
        let store_id = topic_uuid(payload, "store_id")?;
        let Some(account) = self
            .repository
            .resolve_meta_account(store_id)
            .await
            .map_err(TopicEventFailure::from_application_error)?
        else {
            tracing::info!(%store_id, "capi delivery skipped: no enabled meta destination");
            return Ok(());
        };
        // Purchase uses the Order id as its stable event id. chaos-js's Pixel
        // projection uses the same id, so Meta deduplicates the two copies.
        let event_id = topic_uuid(payload, "event_id")?;
        let shopper_id = topic_uuid(payload, "shopper_id")?;
        let occurred_at = payload
            .get("occurred_at")
            .and_then(Value::as_str)
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
            .ok_or_else(|| TopicEventFailure::terminal(topic_field_error("occurred_at")))?;
        let properties = payload.get("properties").cloned().unwrap_or(Value::Null);
        let command = AnalyticsDeliveryCommand {
            provider: account.provider,
            event_id,
            external_account_reference: account.external_account_reference,
            credential_secret_reference: account.credential_secret_reference,
            configuration: account.configuration,
            event_name: "purchase".into(),
            event_source: "server".into(),
            occurred_at,
            shopper_id,
            properties,
        };
        let receipt = self
            .destination
            .send(&command)
            .await
            .map_err(|error| TopicEventFailure {
                message: error.message,
                retryable: error.retryable,
            })?;
        tracing::info!(
            %store_id,
            %event_id,
            event_name = %command.event_name,
            provider_reference = ?receipt.provider_reference,
            "capi delivery sent"
        );
        Ok(())
    }
}

fn topic_uuid(payload: &Value, field: &'static str) -> Result<Uuid, TopicEventFailure> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| TopicEventFailure::terminal(topic_field_error(field)))
}

fn topic_field_error(field: &'static str) -> String {
    format!("commerce event message missing or invalid field {field}")
}

fn is_purchase_topic(routing_key: &str) -> bool {
    routing_key == ORDER_PAYMENT_COMPLETED_TOPIC
}

#[cfg(test)]
mod worker_tests {
    use super::is_purchase_topic;

    #[test]
    fn capi_consumer_accepts_only_completed_purchases() {
        assert!(is_purchase_topic("order.payment.completed"));
        assert!(!is_purchase_topic("cart.item.added"));
        assert!(!is_purchase_topic("order.payment.initiated"));
    }
}
