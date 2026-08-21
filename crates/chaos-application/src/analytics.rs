use std::{collections::BTreeMap, sync::Arc};

use chaos_domain::{
    FieldViolation,
    analytics::{AnalyticsSettings, BrowserCollectionBasis, BrowserCollectionMode, BrowserEvent},
    store::{StoreId, StoreRole},
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    ports::{
        AnalyticsCollectionRateLimiter, AnalyticsConnection, AnalyticsConnectionConfiguration,
        AnalyticsConnectionRepository, AnalyticsDeliveryError, AnalyticsDeliveryRepository,
        AnalyticsEventDestination, AnalyticsEventPage, AnalyticsEventQuery,
        AnalyticsEventQueryRepository, AnalyticsEventRecorderRepository, AnalyticsEventRepository,
        AnalyticsSettingsRepository, IdempotencyRequest, MachineActor, StoreAnalyticsSettings,
    },
    store::StoreActor,
};

const MAX_BATCH_SIZE: usize = 20;
const MAX_PAST_SKEW: Duration = Duration::hours(24);
const MAX_FUTURE_SKEW: Duration = Duration::minutes(5);

pub struct CollectBrowserEventsInput {
    pub actor: MachineActor,
    pub events: Vec<BrowserEvent>,
    pub received_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserEventCollectionResult {
    pub received: usize,
    pub stored: usize,
    pub duplicates: usize,
    pub discarded_for_consent: usize,
    pub discarded_for_settings: usize,
    pub settings_revision: i32,
}

pub struct AnalyticsCollection {
    repository: Arc<dyn AnalyticsEventRepository>,
    rate_limiter: Arc<dyn AnalyticsCollectionRateLimiter>,
}

impl AnalyticsCollection {
    pub fn new(
        repository: Arc<dyn AnalyticsEventRepository>,
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
        let sales_channel_id = input
            .actor
            .sales_channel_id
            .ok_or(ApplicationError::Forbidden)?;
        if input.events.is_empty() || input.events.len() > MAX_BATCH_SIZE {
            return Err(validation("events", "must contain between 1 and 20 events"));
        }
        for event in &input.events {
            if event.occurred_at() < input.received_at - MAX_PAST_SKEW
                || event.occurred_at() > input.received_at + MAX_FUTURE_SKEW
            {
                return Err(validation(
                    "events.occurred_at",
                    "must be within 24 hours before or 5 minutes after receipt",
                ));
            }
        }
        let mut counts = BTreeMap::new();
        for event in &input.events {
            *counts.entry(event.shopper_id()).or_insert(0_u16) += 1;
        }
        let decision = self
            .rate_limiter
            .consume(
                input.actor.store_id,
                sales_channel_id,
                &counts.into_iter().collect::<Vec<_>>(),
                u16::try_from(input.events.len()).expect("bounded Analytics batch"),
            )
            .await?;
        if !decision.allowed {
            return Err(ApplicationError::RateLimited {
                retry_after_seconds: decision.retry_after_seconds,
            });
        }

        let received = input.events.len();
        let settings = self
            .repository
            .resolve_collection_settings(&input.actor, input.received_at)
            .await?;
        let consented = input
            .events
            .into_iter()
            .filter(|event| browser_event_is_collectable(event, settings.settings))
            .collect::<Vec<_>>();
        let discarded_for_consent = received - consented.len();
        let discarded_for_settings = if settings.settings.collection_enabled() {
            0
        } else {
            consented.len()
        };
        let eligible = if settings.settings.collection_enabled() {
            consented
        } else {
            Vec::new()
        };
        let stored = if eligible.is_empty() {
            0
        } else {
            self.repository
                .append_browser_events(
                    &input.actor,
                    &eligible,
                    settings.revision,
                    settings.settings.browser_collection_mode(),
                    settings.settings.provider_reporting_enabled(),
                    input.received_at,
                )
                .await?
        };
        Ok(BrowserEventCollectionResult {
            received,
            stored,
            duplicates: eligible.len() - stored,
            discarded_for_consent,
            discarded_for_settings,
            settings_revision: settings.revision,
        })
    }
}

fn browser_event_is_collectable(event: &BrowserEvent, settings: AnalyticsSettings) -> bool {
    event.consent().analytics_storage()
        || (event.collection_basis() == BrowserCollectionBasis::StorePolicy
            && settings.browser_collection_mode() == BrowserCollectionMode::OptOut)
}

pub struct UpdateAnalyticsSettingsInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub collection_enabled: bool,
    pub browser_collection_mode: BrowserCollectionMode,
    pub provider_reporting_enabled: bool,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct AnalyticsAdministration {
    settings: Arc<dyn AnalyticsSettingsRepository>,
    connections: Arc<dyn AnalyticsConnectionRepository>,
    events: Arc<dyn AnalyticsEventQueryRepository>,
}

impl AnalyticsAdministration {
    pub fn new(
        settings: Arc<dyn AnalyticsSettingsRepository>,
        connections: Arc<dyn AnalyticsConnectionRepository>,
        events: Arc<dyn AnalyticsEventQueryRepository>,
    ) -> Self {
        Self {
            settings,
            connections,
            events,
        }
    }

    pub async fn get_settings(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        _now: OffsetDateTime,
    ) -> Result<StoreAnalyticsSettings, ApplicationError> {
        self.settings
            .get_settings(actor, store_id)
            .await?
            .ok_or_else(|| store_not_found(store_id))
    }

    pub async fn update_settings(
        &self,
        input: UpdateAnalyticsSettingsInput,
    ) -> Result<StoreAnalyticsSettings, ApplicationError> {
        require_owner(input.actor)?;
        let settings = AnalyticsSettings::new(
            input.collection_enabled,
            input.browser_collection_mode,
            input.provider_reporting_enabled,
        )?;
        self.settings
            .update_settings(
                input.actor,
                input.store_id,
                settings,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn get_connection(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsConnection>, ApplicationError> {
        self.connections
            .get_connection(actor, store_id, provider)
            .await
    }

    pub async fn configure_connection(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        configuration: AnalyticsConnectionConfiguration,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsConnection, ApplicationError> {
        require_owner(actor)?;
        self.connections
            .configure_connection(actor, store_id, configuration, idempotency, now)
            .await
    }

    pub async fn list_events(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        query: AnalyticsEventQuery,
        limit: u16,
    ) -> Result<AnalyticsEventPage, ApplicationError> {
        self.events.list_events(actor, store_id, query, limit).await
    }
}

pub struct AnalyticsEventRecorder {
    repository: Arc<dyn AnalyticsEventRecorderRepository>,
}

impl AnalyticsEventRecorder {
    pub fn new(repository: Arc<dyn AnalyticsEventRecorderRepository>) -> Self {
        Self { repository }
    }

    pub async fn run_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self.repository.claim_server_events(limit).await?;
        for job in &jobs {
            let result = self
                .repository
                .ingest_server_event(job, now)
                .await
                .map_err(|error| error.to_string());
            self.repository
                .finish_server_event(job, result, now)
                .await?;
        }
        Ok(jobs.len())
    }
}

pub struct AnalyticsDeliveryWorker {
    repository: Arc<dyn AnalyticsDeliveryRepository>,
    destinations: std::collections::HashMap<String, Arc<dyn AnalyticsEventDestination>>,
}

impl AnalyticsDeliveryWorker {
    pub fn new(
        repository: Arc<dyn AnalyticsDeliveryRepository>,
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
fn require_owner(actor: StoreActor) -> Result<(), ApplicationError> {
    if actor.role() == StoreRole::Owner {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaos_domain::analytics::{BrowserEventProperties, ConsentSnapshot, TrafficAttribution};
    use uuid::Uuid;

    fn event(consented: bool, basis: BrowserCollectionBasis) -> BrowserEvent {
        BrowserEvent::new(
            Uuid::now_v7(),
            1,
            OffsetDateTime::now_utc(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            ConsentSnapshot::new(consented, false, "test-v1").unwrap(),
            basis,
            None::<TrafficAttribution>,
            BrowserEventProperties::page_view("/", None, None).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn store_policy_collection_requires_the_authoritative_store_setting() {
        let policy_event = event(false, BrowserCollectionBasis::StorePolicy);
        let required = AnalyticsSettings::new(true, BrowserCollectionMode::OptIn, false).unwrap();
        let opt_out = AnalyticsSettings::new(true, BrowserCollectionMode::OptOut, false).unwrap();
        assert!(!browser_event_is_collectable(&policy_event, required));
        assert!(browser_event_is_collectable(&policy_event, opt_out));
        assert!(browser_event_is_collectable(
            &event(true, BrowserCollectionBasis::Consent),
            required,
        ));
    }
}
