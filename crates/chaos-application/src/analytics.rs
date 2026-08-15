use std::sync::Arc;

use chaos_domain::{FieldViolation, analytics::BrowserEvent, merchant::ApiKeyClass};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    ports::{AnalyticsEventRepository, MachineActor},
};

const COLLECTION_POLICY_VERSION: &str = "builtin-v1";
const MAX_BATCH_SIZE: usize = 20;
const MAX_PAST_SKEW: Duration = Duration::hours(24);
const MAX_FUTURE_SKEW: Duration = Duration::minutes(5);
const RAW_EVENT_RETENTION: Duration = Duration::days(30);

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
    pub collection_policy_version: &'static str,
}

pub struct AnalyticsCollection {
    repository: Arc<dyn AnalyticsEventRepository>,
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
        let eligible = input
            .events
            .into_iter()
            .filter(|event| event.consent().analytics_storage())
            .collect::<Vec<_>>();
        let discarded_for_consent = received - eligible.len();
        let stored = if eligible.is_empty() {
            0
        } else {
            self.repository
                .append_browser_events(
                    &input.actor,
                    &eligible,
                    COLLECTION_POLICY_VERSION,
                    input.received_at,
                    input.received_at + RAW_EVENT_RETENTION,
                )
                .await?
        };
        Ok(BrowserEventCollectionResult {
            received,
            stored,
            duplicates: eligible.len() - stored,
            discarded_for_consent,
            collection_policy_version: COLLECTION_POLICY_VERSION,
        })
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
