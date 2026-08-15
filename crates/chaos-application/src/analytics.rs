use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    analytics::{AnalyticsPolicy, BrowserEvent, SessionEventContribution},
    merchant::{ApiKeyClass, MerchantRole, StoreId},
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        AnalyticsEventRepository, AnalyticsPolicyRepository, AnalyticsSessionizationQueue,
        IdempotencyRequest, MachineActor, StoreAnalyticsPolicy,
    },
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
    pub discarded_for_policy: usize,
    pub collection_policy_version: String,
}

pub struct AnalyticsCollection {
    repository: Arc<dyn AnalyticsEventRepository>,
}

pub struct AnalyticsWorkers {
    sessionization_queue: Arc<dyn AnalyticsSessionizationQueue>,
}

pub struct UpdateAnalyticsPolicyInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub behavior_collection_enabled: bool,
    pub advertising_exports_enabled: bool,
    pub identity_linking_enabled: bool,
    pub raw_event_retention_days: u16,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct AnalyticsAdministration {
    repository: Arc<dyn AnalyticsPolicyRepository>,
}

impl AnalyticsAdministration {
    pub fn new(repository: Arc<dyn AnalyticsPolicyRepository>) -> Self {
        Self { repository }
    }

    pub async fn get_policy(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        now: OffsetDateTime,
    ) -> Result<StoreAnalyticsPolicy, ApplicationError> {
        self.repository
            .get_store_policy(actor, store_id, now)
            .await?
            .ok_or_else(|| store_not_found(store_id))
    }

    pub async fn update_policy(
        &self,
        input: UpdateAnalyticsPolicyInput,
    ) -> Result<StoreAnalyticsPolicy, ApplicationError> {
        match input.actor.role() {
            MerchantRole::Owner | MerchantRole::Administrator => {}
            MerchantRole::Developer | MerchantRole::Manager | MerchantRole::Support => {
                return Err(ApplicationError::Forbidden);
            }
        }
        let policy = AnalyticsPolicy::new(
            input.behavior_collection_enabled,
            input.advertising_exports_enabled,
            input.identity_linking_enabled,
            input.raw_event_retention_days,
        )?;
        self.repository
            .update_store_policy(
                input.actor,
                input.store_id,
                policy,
                &input.idempotency,
                input.now,
            )
            .await
    }
}

impl AnalyticsWorkers {
    pub fn new(sessionization_queue: Arc<dyn AnalyticsSessionizationQueue>) -> Self {
        Self {
            sessionization_queue,
        }
    }

    pub async fn run_sessionization_batch(
        &self,
        worker_id: uuid::Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .sessionization_queue
            .claim_sessionization(worker_id, limit, now, now - Duration::minutes(1))
            .await?;
        for job in &jobs {
            let contribution = SessionEventContribution::from_event(
                job.event_name,
                job.active_engagement_milliseconds,
            )
            .map_err(|error| error.to_string());
            self.sessionization_queue
                .finish_sessionization(worker_id, job, contribution, now)
                .await?;
        }
        Ok(jobs.len())
    }

    pub async fn run_retention_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<crate::ports::AnalyticsRetentionPurgeResult, ApplicationError> {
        self.sessionization_queue
            .purge_expired_data(limit, now)
            .await
    }
}

impl AnalyticsCollection {
    pub fn new(repository: Arc<dyn AnalyticsEventRepository>) -> Self {
        Self { repository }
    }

    pub async fn collect(
        &self,
        input: CollectBrowserEventsInput,
    ) -> Result<BrowserEventCollectionResult, ApplicationError> {
        if input.actor.class != ApiKeyClass::Publishable || input.actor.sales_channel_id.is_none() {
            return Err(ApplicationError::Forbidden);
        }
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

        let received = input.events.len();
        let policy = self
            .repository
            .resolve_collection_policy(&input.actor, input.received_at)
            .await?;
        let consent_eligible = input
            .events
            .into_iter()
            .filter(|event| event.consent().analytics_storage())
            .collect::<Vec<_>>();
        let discarded_for_consent = received - consent_eligible.len();
        let discarded_for_policy = if policy.policy.behavior_collection_enabled() {
            0
        } else {
            consent_eligible.len()
        };
        let eligible = if policy.policy.behavior_collection_enabled() {
            consent_eligible
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
                    &policy.policy_version,
                    input.received_at,
                    input.received_at
                        + Duration::days(i64::from(policy.policy.raw_event_retention_days())),
                )
                .await?
        };
        Ok(BrowserEventCollectionResult {
            received,
            stored,
            duplicates: eligible.len() - stored,
            discarded_for_consent,
            discarded_for_policy,
            collection_policy_version: policy.policy_version,
        })
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
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chaos_domain::{
        analytics::{BrowserEventProperties, ConsentSnapshot},
        merchant::{ApiKeyId, ApiKeyMode, ApiKeyScope, MerchantAccountId, SalesChannelId, StoreId},
    };
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct RecordingRepository(Mutex<Vec<Uuid>>);

    #[async_trait]
    impl AnalyticsEventRepository for RecordingRepository {
        async fn resolve_collection_policy(
            &self,
            _actor: &MachineActor,
            _now: OffsetDateTime,
        ) -> Result<crate::ports::ResolvedAnalyticsPolicy, ApplicationError> {
            Ok(crate::ports::ResolvedAnalyticsPolicy {
                policy: AnalyticsPolicy::builtin(),
                policy_version: "builtin-v1".into(),
            })
        }

        async fn append_browser_events(
            &self,
            _actor: &MachineActor,
            events: &[BrowserEvent],
            _collection_policy_version: &str,
            _received_at: OffsetDateTime,
            _retention_expires_at: OffsetDateTime,
        ) -> Result<usize, ApplicationError> {
            self.0
                .lock()
                .unwrap()
                .extend(events.iter().map(BrowserEvent::event_id));
            Ok(events.len())
        }
    }

    fn actor() -> MachineActor {
        MachineActor {
            api_key_id: ApiKeyId::new(),
            merchant_account_id: MerchantAccountId::new(),
            store_id: StoreId::new(),
            sales_channel_id: Some(SalesChannelId::new()),
            class: ApiKeyClass::Publishable,
            mode: ApiKeyMode::Live,
            scopes: vec![ApiKeyScope::AnalyticsWrite],
        }
    }

    fn event(now: OffsetDateTime, analytics_storage: bool) -> BrowserEvent {
        BrowserEvent::new(
            Uuid::now_v7(),
            1,
            now,
            Uuid::now_v7(),
            Uuid::now_v7(),
            ConsentSnapshot::new(analytics_storage, false, "cmp-v1").unwrap(),
            BrowserEventProperties::page_viewed("/products", None, None).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn collection_discards_events_without_storage_consent() {
        let repository = Arc::new(RecordingRepository::default());
        let collection = AnalyticsCollection::new(repository.clone());
        let now = OffsetDateTime::now_utc();
        let result = collection
            .collect(CollectBrowserEventsInput {
                actor: actor(),
                events: vec![event(now, true), event(now, false)],
                received_at: now,
            })
            .await
            .unwrap();

        assert_eq!(result.stored, 1);
        assert_eq!(result.discarded_for_consent, 1);
        assert_eq!(repository.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn collection_rejects_unbounded_batches_and_timestamp_skew() {
        let collection = AnalyticsCollection::new(Arc::new(RecordingRepository::default()));
        let now = OffsetDateTime::now_utc();
        assert!(
            collection
                .collect(CollectBrowserEventsInput {
                    actor: actor(),
                    events: Vec::new(),
                    received_at: now,
                })
                .await
                .is_err()
        );
        assert!(
            collection
                .collect(CollectBrowserEventsInput {
                    actor: actor(),
                    events: vec![event(now - Duration::hours(25), true)],
                    received_at: now,
                })
                .await
                .is_err()
        );
    }
}
