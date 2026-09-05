use std::{collections::HashMap, sync::Arc};

use crate::{
    ApplicationError,
    adapters::postgres::PostgresShippingRepository,
    contracts::{IntegrationQueue, ShippingProvider},
};
use time::OffsetDateTime;
use uuid::Uuid;

/// Shipping dispatch is separate from Fulfillment state transitions. A
/// carrier adapter can acknowledge a shipment here while Commerce remains the
/// source of truth for `FulfillmentStatus` and the Order shipping projection.
pub struct ShippingWorkers {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<PostgresShippingRepository>,
    providers: HashMap<String, Arc<dyn ShippingProvider>>,
}

const SHIPPING_COMMANDS_QUEUE: &str = "shipping_commands_queue";

impl ShippingWorkers {
    pub fn new(
        queue: Arc<dyn IntegrationQueue>,
        repository: Arc<PostgresShippingRepository>,
        providers: impl IntoIterator<Item = Arc<dyn ShippingProvider>>,
    ) -> Self {
        Self {
            queue,
            repository,
            providers: providers
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
        }
    }

    pub async fn run_outbox_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_topic(SHIPPING_COMMANDS_QUEUE, limit)
            .await?;
        for job in &jobs {
            let result = self
                .execute(&job.payload, now)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_topic(SHIPPING_COMMANDS_QUEUE, job.msg_id, job.attempts, result)
                .await?;
        }
        Ok(jobs.len())
    }

    async fn execute(
        &self,
        payload: &serde_json::Value,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let store_id = topic_uuid(payload, "store_id")?;
        let (provider_name, command) = self
            .repository
            .prepare_shipped_command(store_id, payload)
            .await?;
        let provider =
            self.providers
                .get(&provider_name)
                .ok_or_else(|| ApplicationError::Conflict {
                    code: "shipping_provider_not_supported",
                    message: "the configured Shipping provider has no adapter",
                })?;
        let result = provider.execute(command).await?;
        self.repository
            .record_result(store_id, payload, &result, now)
            .await
    }
}

fn topic_uuid(payload: &serde_json::Value, field: &'static str) -> Result<Uuid, ApplicationError> {
    payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "commerce event message missing or invalid field {field}"
            ))
        })
}
