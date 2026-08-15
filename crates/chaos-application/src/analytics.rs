use std::collections::BTreeMap;
use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    analytics::{AnalyticsPolicy, BrowserEvent, ConsentSnapshot, SessionEventContribution},
    merchant::{ApiKeyClass, ApiKeyScope, MerchantRole, StoreId},
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        AnalyticsCollectionRateLimiter, AnalyticsCommerceFactQueue, AnalyticsErasureRequest,
        AnalyticsErasureSelector, AnalyticsEventRepository, AnalyticsIdentityLink,
        AnalyticsPolicyRepository, AnalyticsPrivacyRepository, AnalyticsSessionizationQueue,
        CustomerActor, IdempotencyRequest, MachineActor, StoreAnalyticsPolicy,
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
    rate_limiter: Arc<dyn AnalyticsCollectionRateLimiter>,
}

pub struct AnalyticsWorkers {
    sessionization_queue: Arc<dyn AnalyticsSessionizationQueue>,
    privacy_repository: Arc<dyn AnalyticsPrivacyRepository>,
    commerce_fact_queue: Arc<dyn AnalyticsCommerceFactQueue>,
}

pub struct LinkAnalyticsIdentityInput {
    pub actor: CustomerActor,
    pub anonymous_id: uuid::Uuid,
    pub consent: ConsentSnapshot,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct RequestAnalyticsErasureInput {
    pub actor: MerchantActor,
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
    ) -> Result<AnalyticsIdentityLink, ApplicationError> {
        let machine = &input.actor.machine;
        if machine.class != ApiKeyClass::Publishable
            || machine.sales_channel_id.is_none()
            || !machine.scopes.contains(&ApiKeyScope::AnalyticsWrite)
        {
            return Err(ApplicationError::Forbidden);
        }
        if input.anonymous_id.is_nil() {
            return Err(validation(
                "anonymous_id",
                "must be a non-zero opaque identifier",
            ));
        }
        if !input.consent.analytics_storage() {
            return Err(validation(
                "consent.analytics_storage",
                "must be granted before identity linking",
            ));
        }
        self.repository
            .link_customer_identity(
                &input.actor,
                input.anonymous_id,
                &input.consent,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn request_erasure(
        &self,
        input: RequestAnalyticsErasureInput,
    ) -> Result<AnalyticsErasureRequest, ApplicationError> {
        require_privacy_administrator(input.actor)?;
        let selector_id = match input.selector {
            AnalyticsErasureSelector::Anonymous(id) => id,
            AnalyticsErasureSelector::Customer(id) => id.as_uuid(),
        };
        if selector_id.is_nil() {
            return Err(validation("selector", "must contain a non-zero identifier"));
        }
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
        actor: MerchantActor,
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
    pub fn new(
        sessionization_queue: Arc<dyn AnalyticsSessionizationQueue>,
        privacy_repository: Arc<dyn AnalyticsPrivacyRepository>,
        commerce_fact_queue: Arc<dyn AnalyticsCommerceFactQueue>,
    ) -> Self {
        Self {
            sessionization_queue,
            privacy_repository,
            commerce_fact_queue,
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

    pub async fn run_erasure_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<crate::ports::AnalyticsErasureBatchResult, ApplicationError> {
        self.privacy_repository
            .process_erasure_requests(limit, now)
            .await
    }

    pub async fn run_commerce_fact_batch(
        &self,
        worker_id: uuid::Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .commerce_fact_queue
            .claim_commerce_facts(worker_id, limit, now, now - Duration::minutes(1))
            .await?;
        for job in &jobs {
            let result = self
                .commerce_fact_queue
                .ingest_commerce_fact(job, now)
                .await
                .map_err(|error| error.to_string());
            self.commerce_fact_queue
                .finish_commerce_fact(worker_id, job.id, result, now)
                .await?;
        }
        Ok(jobs.len())
    }
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

        let sales_channel_id = input
            .actor
            .sales_channel_id
            .ok_or(ApplicationError::Forbidden)?;
        let mut counts = BTreeMap::<uuid::Uuid, u16>::new();
        for event in &input.events {
            *counts.entry(event.anonymous_id()).or_default() += 1;
        }
        let anonymous_event_counts = counts.into_iter().collect::<Vec<_>>();
        let decision = self
            .rate_limiter
            .consume(
                input.actor.merchant_account_id.as_uuid(),
                input.actor.store_id,
                sales_channel_id,
                &anonymous_event_counts,
                u16::try_from(input.events.len()).expect("validated analytics batch fits in u16"),
            )
            .await?;
        if !decision.allowed {
            return Err(ApplicationError::RateLimited {
                retry_after_seconds: decision.retry_after_seconds,
            });
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

fn require_privacy_administrator(actor: MerchantActor) -> Result<(), ApplicationError> {
    match actor.role() {
        MerchantRole::Owner | MerchantRole::Administrator => Ok(()),
        MerchantRole::Developer | MerchantRole::Manager | MerchantRole::Support => {
            Err(ApplicationError::Forbidden)
        }
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

    struct RateLimiter(bool);

    #[async_trait]
    impl AnalyticsCollectionRateLimiter for RateLimiter {
        async fn consume(
            &self,
            _merchant_account_id: Uuid,
            _store_id: StoreId,
            _sales_channel_id: SalesChannelId,
            _anonymous_event_counts: &[(Uuid, u16)],
            _event_count: u16,
        ) -> Result<crate::ports::AnalyticsRateLimitDecision, ApplicationError> {
            Ok(crate::ports::AnalyticsRateLimitDecision {
                allowed: self.0,
                retry_after_seconds: 60,
            })
        }
    }

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
        let collection = AnalyticsCollection::new(repository.clone(), Arc::new(RateLimiter(true)));
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
        let collection = AnalyticsCollection::new(
            Arc::new(RecordingRepository::default()),
            Arc::new(RateLimiter(true)),
        );
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

    #[tokio::test]
    async fn collection_rejects_a_rate_limited_batch_before_persistence() {
        let repository = Arc::new(RecordingRepository::default());
        let collection = AnalyticsCollection::new(repository.clone(), Arc::new(RateLimiter(false)));
        let now = OffsetDateTime::now_utc();
        let error = collection
            .collect(CollectBrowserEventsInput {
                actor: actor(),
                events: vec![event(now, true)],
                received_at: now,
            })
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ApplicationError::RateLimited {
                retry_after_seconds: 60
            }
        ));
        assert!(repository.0.lock().unwrap().is_empty());
    }
}
