use std::{collections::HashMap, sync::Arc};

use crate::{
    ApplicationError,
    adapters::postgres::PostgresShippingRepository,
    contracts::{IntegrationQueue, ShippingProvider},
};
use time::OffsetDateTime;

/// Shipping dispatch is separate from Fulfillment state transitions. A
/// carrier adapter can acknowledge a shipment here while Commerce remains the
/// source of truth for `FulfillmentStatus` and the Order shipping projection.
pub struct ShippingWorkers {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<PostgresShippingRepository>,
    providers: HashMap<String, Arc<dyn ShippingProvider>>,
}

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
            .claim_outbox("chaos_shipping_commands", limit)
            .await?;
        for job in &jobs {
            let result = self
                .execute(job, now)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_outbox(job.id, job.attempts, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    async fn execute(
        &self,
        job: &crate::contracts::QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if job.internal_event_type.as_deref() != Some("fulfillment.shipped") {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "unsupported shipping event {}",
                job.internal_event_type.as_deref().unwrap_or("unknown")
            )));
        }
        let (provider_name, command) = self.repository.prepare_shipped_command(job).await?;
        let provider =
            self.providers
                .get(&provider_name)
                .ok_or_else(|| ApplicationError::Conflict {
                    code: "shipping_provider_not_supported",
                    message: "the configured Shipping provider has no adapter",
                })?;
        let result = provider.execute(command).await?;
        self.repository.record_result(job, &result, now).await
    }
}
