use std::{collections::BTreeMap, sync::Arc};

use chaos_domain::{
    FieldViolation,
    analytics::{
        AnalyticsSettings, BrowserCollectionBasis, BrowserCollectionMode, BrowserEvent,
        ConsentSnapshot,
    },
    store::{StoreId, StoreRole},
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    ports::{
        AnalyticsCollectionRateLimiter, AnalyticsErasureRequest, AnalyticsErasureSelector,
        AnalyticsEventRepository, AnalyticsPrivacyRepository, AnalyticsSettingsRepository,
        AnalyticsWorkerRepository, CustomerActor, IdempotencyRequest, MachineActor, MetaConnection,
        MetaConnectionConfiguration, MetaConnectionRepository, MetaEventDestination,
        StoreAnalyticsSettings, VisitorCustomerLink,
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
            *counts.entry(event.visitor_id()).or_insert(0_u16) += 1;
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
                    settings.settings.meta_reporting_enabled(),
                    input.received_at,
                    input.received_at
                        + Duration::days(i64::from(settings.settings.raw_event_retention_days())),
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

pub struct LinkAnalyticsIdentityInput {
    pub actor: CustomerActor,
    pub visitor_id: uuid::Uuid,
    pub consent: ConsentSnapshot,
    pub collection_basis: BrowserCollectionBasis,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct RequestAnalyticsErasureInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub selector: AnalyticsErasureSelector,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct AnalyticsPrivacy {
    repository: Arc<dyn AnalyticsPrivacyRepository>,
}

impl AnalyticsPrivacy {
    pub fn new(repository: Arc<dyn AnalyticsPrivacyRepository>) -> Self {
        Self { repository }
    }

    pub async fn link_identity(
        &self,
        input: LinkAnalyticsIdentityInput,
    ) -> Result<VisitorCustomerLink, ApplicationError> {
        if input.actor.machine.sales_channel_id.is_none() {
            return Err(ApplicationError::Forbidden);
        }
        if input.visitor_id.is_nil()
            || (!input.consent.analytics_storage()
                && input.collection_basis != BrowserCollectionBasis::StorePolicy)
        {
            return Err(validation(
                "visitor_id",
                "requires a non-zero identifier and consent",
            ));
        }
        self.repository
            .link_visitor_to_customer(
                &input.actor,
                input.visitor_id,
                input.consent.policy_version(),
                input.consent.advertising_storage(),
                input.collection_basis,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn request_erasure(
        &self,
        input: RequestAnalyticsErasureInput,
    ) -> Result<AnalyticsErasureRequest, ApplicationError> {
        require_owner(input.actor)?;
        self.repository
            .request_erasure(
                input.actor,
                input.store_id,
                input.selector,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn get_erasure_request(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        request_id: uuid::Uuid,
    ) -> Result<AnalyticsErasureRequest, ApplicationError> {
        self.repository
            .get_erasure_request(actor, store_id, request_id)
            .await?
            .ok_or(ApplicationError::NotFound {
                resource: "analytics_erasure_request",
                id: request_id.to_string(),
            })
    }
}

pub struct UpdateAnalyticsSettingsInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub collection_enabled: bool,
    pub browser_collection_mode: BrowserCollectionMode,
    pub meta_reporting_enabled: bool,
    pub identity_linking_enabled: bool,
    pub raw_event_retention_days: u16,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct AnalyticsAdministration {
    settings: Arc<dyn AnalyticsSettingsRepository>,
    meta: Arc<dyn MetaConnectionRepository>,
}

impl AnalyticsAdministration {
    pub fn new(
        settings: Arc<dyn AnalyticsSettingsRepository>,
        meta: Arc<dyn MetaConnectionRepository>,
    ) -> Self {
        Self { settings, meta }
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
            input.meta_reporting_enabled,
            input.identity_linking_enabled,
            input.raw_event_retention_days,
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

    pub async fn get_meta_connection(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<Option<MetaConnection>, ApplicationError> {
        self.meta.get_meta_connection(actor, store_id).await
    }

    pub async fn configure_meta_connection(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        configuration: MetaConnectionConfiguration,
        idempotency: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<MetaConnection, ApplicationError> {
        require_owner(actor)?;
        self.meta
            .configure_meta_connection(actor, store_id, configuration, idempotency, now)
            .await
    }
}

pub struct AnalyticsWorkers {
    repository: Arc<dyn AnalyticsWorkerRepository>,
    meta: Arc<dyn MetaEventDestination>,
}

impl AnalyticsWorkers {
    pub fn new(
        repository: Arc<dyn AnalyticsWorkerRepository>,
        meta: Arc<dyn MetaEventDestination>,
    ) -> Self {
        Self { repository, meta }
    }

    pub async fn run_server_event_batch(
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

    pub async fn run_meta_delivery_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self.repository.claim_meta_deliveries(limit).await?;
        for job in &jobs {
            let result = match self.repository.load_meta_delivery(job).await {
                Ok(command) => self.meta.send(&command).await,
                Err(error) => Err(crate::ports::MetaDeliveryError {
                    retryable: false,
                    message: error.to_string(),
                }),
            };
            self.repository
                .finish_meta_delivery(job, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    pub async fn run_retention_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<crate::ports::AnalyticsRetentionResult, ApplicationError> {
        self.repository.purge_expired(limit, now).await
    }

    pub async fn run_erasure_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<crate::ports::AnalyticsErasureBatchResult, ApplicationError> {
        self.repository.process_erasure_requests(limit, now).await
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
    use chaos_domain::analytics::{BrowserEventProperties, TrafficAttribution};
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
        let required =
            AnalyticsSettings::new(true, BrowserCollectionMode::OptIn, false, false, 30).unwrap();
        let opt_out =
            AnalyticsSettings::new(true, BrowserCollectionMode::OptOut, false, false, 30).unwrap();
        assert!(!browser_event_is_collectable(&policy_event, required));
        assert!(browser_event_is_collectable(&policy_event, opt_out));
        assert!(browser_event_is_collectable(
            &event(true, BrowserCollectionBasis::Consent),
            required,
        ));
    }
}
