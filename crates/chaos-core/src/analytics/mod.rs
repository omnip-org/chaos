use std::sync::Arc;

use crate::{
    ApplicationError,
    adapters::postgres::{
        PostgresAnalyticsDeliveryStore, PostgresAnalyticsDestinationStore,
        PostgresAnalyticsEventStore,
    },
    contracts::{
        AnalyticsDeliveryError, AnalyticsDestination, AnalyticsDestinationConfiguration,
        AnalyticsEventDestination, AnalyticsEventPage, AnalyticsEventQuery,
    },
    store::StoreActor,
};
use time::OffsetDateTime;

pub struct AnalyticsAdministration {
    destinations: Arc<PostgresAnalyticsDestinationStore>,
    events: Arc<PostgresAnalyticsEventStore>,
}

impl AnalyticsAdministration {
    pub fn new(
        destinations: Arc<PostgresAnalyticsDestinationStore>,
        events: Arc<PostgresAnalyticsEventStore>,
    ) -> Self {
        Self {
            destinations,
            events,
        }
    }

    pub async fn get_destination(
        &self,
        actor: StoreActor,
        store_id: chaos_domain::store::StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsDestination>, ApplicationError> {
        self.destinations
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
        self.destinations
            .configure_destination(actor, store_id, configuration, now)
            .await
    }

    pub async fn list_events(
        &self,
        actor: StoreActor,
        store_id: chaos_domain::store::StoreId,
        query: AnalyticsEventQuery,
        limit: u16,
    ) -> Result<AnalyticsEventPage, ApplicationError> {
        if query.before_id.is_some() != query.before_received_at.is_some() {
            return Err(validation(
                "before_id",
                "before_id and before_received_at must be provided together",
            ));
        }
        self.events.list_events(actor, store_id, query, limit).await
    }
}

pub struct AnalyticsDeliveryWorker {
    repository: Arc<PostgresAnalyticsDeliveryStore>,
    destinations: std::collections::HashMap<String, Arc<dyn AnalyticsEventDestination>>,
}

impl AnalyticsDeliveryWorker {
    pub fn new(
        repository: Arc<PostgresAnalyticsDeliveryStore>,
        destinations: impl IntoIterator<Item = Arc<dyn AnalyticsEventDestination>>,
    ) -> Self {
        Self {
            repository,
            destinations: destinations
                .into_iter()
                .map(|destination| (destination.provider().to_owned(), destination))
                .collect(),
        }
    }

    pub async fn run_delivery_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let scheduled = self.repository.schedule_deliveries(limit).await?;
        let jobs = self.repository.claim_deliveries(limit).await?;
        for job in &jobs {
            let result = match self.repository.load_delivery(job).await {
                Ok(command) => match self.destinations.get(&command.provider) {
                    Some(destination) => destination.send(&command).await,
                    None => Err(AnalyticsDeliveryError {
                        retryable: false,
                        message: format!(
                            "analytics provider {} is not configured",
                            command.provider
                        ),
                    }),
                },
                Err(error) => Err(AnalyticsDeliveryError {
                    retryable: false,
                    message: error.to_string(),
                }),
            };
            self.repository.finish_delivery(job, result, now).await?;
        }
        Ok(scheduled + jobs.len())
    }
}

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}
