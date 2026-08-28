use std::sync::Arc;

use crate::{
    ApplicationError,
    adapters::postgres::{
        PostgresAnalyticsDeliveryStore, PostgresAnalyticsDestinationStore,
        PostgresAnalyticsEventStore,
    },
    contracts::{
        AnalyticsCollectionRateLimiter, AnalyticsDeliveryError, AnalyticsDestination,
        AnalyticsDestinationConfiguration, AnalyticsEventDestination, AnalyticsEventInput,
        AnalyticsEventPage, AnalyticsEventQuery, MachineActor,
    },
    store::StoreActor,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const MAX_BATCH_SIZE: usize = 20;
const MAX_PAST_SKEW: Duration = Duration::hours(24);
const MAX_FUTURE_SKEW: Duration = Duration::minutes(5);
const MAX_EVENT_NAME_BYTES: usize = 64;
const MAX_PROPERTIES_BYTES: usize = 32 * 1024;

pub struct CollectBrowserEventsInput {
    pub actor: MachineActor,
    pub shopper_id: Uuid,
    pub events: Vec<AnalyticsEventInput>,
    pub received_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserEventCollectionResult {
    pub received: usize,
    pub stored: usize,
    pub duplicates: usize,
}

pub struct AnalyticsCollection {
    repository: Arc<PostgresAnalyticsEventStore>,
    rate_limiter: Arc<dyn AnalyticsCollectionRateLimiter>,
}

impl AnalyticsCollection {
    pub fn new(
        repository: Arc<PostgresAnalyticsEventStore>,
        rate_limiter: Arc<dyn AnalyticsCollectionRateLimiter>,
    ) -> Self {
        Self {
            repository,
            rate_limiter,
        }
    }

    pub async fn collect(
        &self,
        input: CollectBrowserEventsInput,
    ) -> Result<BrowserEventCollectionResult, ApplicationError> {
        if input.events.is_empty() || input.events.len() > MAX_BATCH_SIZE {
            return Err(validation("events", "must contain between 1 and 20 events"));
        }
        for event in &input.events {
            validate_event(event, input.received_at)?;
        }
        let decision = self
            .rate_limiter
            .consume(
                input.actor.store_id,
                input.shopper_id,
                u16::try_from(input.events.len()).expect("bounded Analytics batch"),
            )
            .await?;
        if !decision.allowed {
            return Err(ApplicationError::RateLimited {
                retry_after_seconds: decision.retry_after_seconds,
            });
        }

        let received = input.events.len();
        let stored = self
            .repository
            .append_events(
                &input.actor,
                input.shopper_id,
                &input.events,
                input.received_at,
            )
            .await?;
        Ok(BrowserEventCollectionResult {
            received,
            stored,
            duplicates: received - stored,
        })
    }
}

fn validate_event(
    event: &AnalyticsEventInput,
    received_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    if event.event_id.is_nil() {
        return Err(validation("events.event_id", "must be a non-nil UUID"));
    }
    if event.event_name.is_empty()
        || event.event_name.len() > MAX_EVENT_NAME_BYTES
        || !event.event_name.bytes().enumerate().all(|(index, byte)| {
            (index == 0 && byte.is_ascii_lowercase())
                || (index > 0
                    && (byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
        })
    {
        return Err(validation(
            "events.event_name",
            "must be 1-64 lowercase snake_case bytes",
        ));
    }
    if event.event_name == "purchase" {
        return Err(validation(
            "events.event_name",
            "must be recorded by payment confirmation, not generic browser analytics",
        ));
    }
    if !event.properties.is_object() {
        return Err(validation("events.properties", "must be a JSON object"));
    }
    if event.properties.to_string().len() > MAX_PROPERTIES_BYTES {
        return Err(validation(
            "events.properties",
            "must not exceed 32768 bytes",
        ));
    }
    if event.occurred_at < received_at - MAX_PAST_SKEW
        || event.occurred_at > received_at + MAX_FUTURE_SKEW
    {
        return Err(validation(
            "events.occurred_at",
            "must be within 24 hours before or 5 minutes after receipt",
        ));
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_unknown_event_names_with_object_properties() {
        let now = OffsetDateTime::now_utc();
        assert!(
            validate_event(
                &AnalyticsEventInput {
                    event_id: Uuid::now_v7(),
                    event_name: "store_defined_event".into(),
                    occurred_at: now,
                    properties: json!({"product_id": Uuid::now_v7()}),
                },
                now,
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_non_object_properties_only() {
        let now = OffsetDateTime::now_utc();
        assert!(
            validate_event(
                &AnalyticsEventInput {
                    event_id: Uuid::now_v7(),
                    event_name: "page_view".into(),
                    occurred_at: now,
                    properties: json!(["not-an-object"]),
                },
                now,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_client_purchase_from_browser_collection() {
        let now = OffsetDateTime::now_utc();
        assert!(
            validate_event(
                &AnalyticsEventInput {
                    event_id: Uuid::now_v7(),
                    event_name: "purchase".into(),
                    occurred_at: now,
                    properties: json!({}),
                },
                now,
            )
            .is_err()
        );
    }

    #[test]
    fn accepts_client_commerce_events_after_their_operations() {
        let now = OffsetDateTime::now_utc();
        for event_name in ["add_to_cart", "initiate_checkout"] {
            assert!(
                validate_event(
                    &AnalyticsEventInput {
                        event_id: Uuid::now_v7(),
                        event_name: event_name.into(),
                        occurred_at: now,
                        properties: json!({}),
                    },
                    now,
                )
                .is_ok(),
                "{event_name} is collected after a successful commerce operation"
            );
        }
    }
}
