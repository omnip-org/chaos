use std::sync::Arc;

use time::OffsetDateTime;

use crate::{ApplicationError, ports::ShippingEventQueue};

/// Applies shipping-provider callbacks to the order aggregate.
pub struct ShippingEventWorkers {
    queue: Arc<dyn ShippingEventQueue>,
}

impl ShippingEventWorkers {
    pub fn new(queue: Arc<dyn ShippingEventQueue>) -> Self {
        Self { queue }
    }

    pub async fn run_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self.queue.claim_events(limit).await?;
        for job in &jobs {
            let result = self
                .queue
                .process_event(job, now)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_event(job.id, job.attempts, result, now)
                .await?;
        }
        Ok(jobs.len())
    }
}
