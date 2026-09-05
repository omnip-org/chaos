use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::{
        PostgresAnalyticsDestinationStore, PostgresAnalyticsEventStore, PostgresCapiEventStore,
    },
    contracts::{
        AnalyticsDestination, AnalyticsDestinationConfiguration, AnalyticsEventDestination,
        AnalyticsEventPage, AnalyticsEventQuery, IntegrationQueue,
    },
    store::StoreActor,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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

/// Consumes `analytics_capi_queue` (bound to the `payment.initiated`/
/// `payment.completed` routing keys — see `migrations/0011_topic_routing.sql`)
/// and delivers to the one configured Meta CAPI destination. Topic routing
/// already picked this consumer, so there's no provider-name dispatch here
/// the way a shared queue would need; a second ad-platform destination
/// would get its own queue, binding, and worker instance instead of joining
/// this one.
pub struct MetaCapiWorker {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<PostgresCapiEventStore>,
    destination: Arc<dyn AnalyticsEventDestination>,
}

const CAPI_QUEUE: &str = "analytics_capi_queue";

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
        let jobs = self.queue.claim_topic(CAPI_QUEUE, limit).await?;
        for job in &jobs {
            let result = self.deliver(&job.payload).await;
            if let Err(error) = &result {
                tracing::warn!(error = %error, "capi delivery failed");
            }
            self.queue
                .finish_topic(
                    CAPI_QUEUE,
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
        let analytics_event_id = topic_uuid(payload, "analytics_event_id")?;
        let received_at = payload
            .get("received_at")
            .and_then(Value::as_str)
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
            .ok_or_else(|| topic_field_error("received_at"))?;
        let Some(command) = self
            .repository
            .load_command(store_id, analytics_event_id, received_at)
            .await?
        else {
            return Ok(());
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

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}
