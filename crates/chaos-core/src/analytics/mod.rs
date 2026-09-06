use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresCapiEventStore,
    contracts::{
        ANALYTICS_CAPI_QUEUE, AnalyticsDeliveryCommand, AnalyticsDestination,
        AnalyticsDestinationConfiguration, AnalyticsEventDestination, IntegrationQueue,
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

/// Consumes `analytics_capi_queue` (bound to `cart.item.added`,
/// `order.payment.initiated`, and `order.payment.completed` — see
/// `migrations/0004_integration.sql`) and delivers to the one configured
/// Meta CAPI destination. Topic routing
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
            let result = self.deliver(&job.payload).await;
            if let Err(error) = &result {
                tracing::warn!(error = %error, "capi delivery failed");
            }
            self.queue
                .finish_topic(
                    ANALYTICS_CAPI_QUEUE,
                    job.msg_id,
                    job.attempts,
                    result.map_err(|error| error.to_string()),
                )
                .await?;
        }
        Ok(jobs.len())
    }

    async fn deliver(&self, payload: &Value) -> Result<(), ApplicationError> {
        let store_id = topic_uuid(payload, "store_id")?;
        let Some(account) = self.repository.resolve_meta_account(store_id).await? else {
            return Ok(());
        };
        // Every analytics topic payload carries an explicit event_id: for the
        // payment keys it equals the Order id, for cart.item.added it is minted
        // in the cart transaction. chaos-js's Pixel projection reuses the same
        // id, so Meta dedupes the two copies.
        let event_id = topic_uuid(payload, "event_id")?;
        let shopper_id = topic_uuid(payload, "shopper_id")?;
        let event_name = payload
            .get("event_name")
            .and_then(Value::as_str)
            .ok_or_else(|| topic_field_error("event_name"))?
            .to_owned();
        let occurred_at = payload
            .get("occurred_at")
            .and_then(Value::as_str)
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
            .ok_or_else(|| topic_field_error("occurred_at"))?;
        let properties = payload.get("properties").cloned().unwrap_or(Value::Null);
        let command = AnalyticsDeliveryCommand {
            provider: account.provider,
            event_id,
            external_account_reference: account.external_account_reference,
            credential_secret_reference: account.credential_secret_reference,
            configuration: account.configuration,
            event_name,
            event_source: "server".into(),
            occurred_at,
            shopper_id,
            properties,
        };
        self.destination
            .send(&command)
            .await
            .map_err(|error| ApplicationError::Unexpected(anyhow::anyhow!(error.message)))?;
        Ok(())
    }
}

fn topic_uuid(payload: &Value, field: &'static str) -> Result<Uuid, ApplicationError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| topic_field_error(field))
}

fn topic_field_error(field: &'static str) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "commerce event message missing or invalid field {field}"
    ))
}
