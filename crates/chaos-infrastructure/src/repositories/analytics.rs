use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        AnalyticsAttributionJob, AnalyticsAttributionQueue, AnalyticsCommerceFactJob,
        AnalyticsCommerceFactQueue, AnalyticsDailyReports, AnalyticsErasureBatchResult,
        AnalyticsErasureRequest, AnalyticsErasureSelector, AnalyticsErasureStatus,
        AnalyticsEventRepository, AnalyticsIdentityLink, AnalyticsPolicyRepository,
        AnalyticsPrivacyRepository, AnalyticsReportingRepository, AnalyticsRetentionPurgeResult,
        AnalyticsSessionizationJob, AnalyticsSessionizationQueue, CustomerActor,
        DailyAttributionReport, DailyBehaviorReport, DailyCommerceReport, IdempotencyRequest,
        MachineActor, ResolvedAnalyticsPolicy, StoreAnalyticsPolicy,
    },
};
use chaos_domain::{
    analytics::{
        AnalyticsPolicy, BrowserEvent, BrowserEventName, BrowserEventProperties, ConsentSnapshot,
        SESSION_INACTIVITY_MINUTES, SessionEventContribution, capped_session_engagement,
    },
    identity::UserId,
    merchant::{SalesChannelId, StoreId},
    sales::CustomerId,
};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const UPDATE_POLICY_OPERATION: &str = "analytics_policies.update.v1";
const LINK_IDENTITY_OPERATION: &str = "analytics_identity_links.create.v1";
const REQUEST_ERASURE_OPERATION: &str = "analytics_erasure_requests.create.v1";

#[derive(Clone)]
pub struct PostgresAnalyticsEventRepository {
    pool: PgPool,
}

#[derive(Clone)]
pub struct PostgresAnalyticsReportingRepository {
    pool: PgPool,
}

impl PostgresAnalyticsReportingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(FromRow)]
struct DailyBehaviorReportRow {
    sales_channel_id: Uuid,
    report_date: time::Date,
    sessions: i64,
    events: i64,
    page_views: i64,
    product_views: i64,
    searches: i64,
    cart_line_additions: i64,
    checkouts_started: i64,
    active_engagement_milliseconds: i64,
    refreshed_at: OffsetDateTime,
}

#[derive(FromRow)]
struct DailyCommerceReportRow {
    sales_channel_id: Uuid,
    report_date: time::Date,
    currency: String,
    orders_created: i64,
    order_amount_minor: i64,
    payments_captured: i64,
    captured_amount_minor: i64,
    refunds_succeeded: i64,
    refunded_amount_minor: i64,
    fulfillments_shipped: i64,
    returns_completed: i64,
    refreshed_at: OffsetDateTime,
}

#[derive(FromRow)]
struct DailyAttributionReportRow {
    sales_channel_id: Uuid,
    report_date: time::Date,
    attribution_model: String,
    model_version: i16,
    is_direct: bool,
    campaign_source: String,
    campaign_medium: String,
    campaign_name: String,
    attributed_orders: i64,
    attributed_amount_minor: i64,
    currency: String,
    refreshed_at: OffsetDateTime,
}

#[async_trait]
impl AnalyticsReportingRepository for PostgresAnalyticsReportingRepository {
    async fn list_daily_reports(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        from: time::Date,
        to: time::Date,
    ) -> Result<Option<AnalyticsDailyReports>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            Some(actor.user_id().as_uuid()),
        )
        .await?;
        let store_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM merchant.stores \
             WHERE merchant_account_id = $1 AND id = $2)",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !store_exists {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        }
        let behavior = sqlx::query_as::<_, DailyBehaviorReportRow>(
            "SELECT sales_channel_id, report_date, sessions, events, page_views, product_views, \
                    searches, cart_line_additions, checkouts_started, \
                    active_engagement_milliseconds, refreshed_at \
             FROM analytics.daily_behavior_reports \
             WHERE merchant_account_id = $1 AND store_id = $2 \
               AND report_date BETWEEN $3 AND $4 \
             ORDER BY report_date, sales_channel_id LIMIT 5001",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(from)
        .bind(to)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let commerce = sqlx::query_as::<_, DailyCommerceReportRow>(
            "SELECT sales_channel_id, report_date, currency::text AS currency, orders_created, \
                    order_amount_minor, payments_captured, captured_amount_minor, \
                    refunds_succeeded, refunded_amount_minor, fulfillments_shipped, \
                    returns_completed, refreshed_at \
             FROM analytics.daily_commerce_reports \
             WHERE merchant_account_id = $1 AND store_id = $2 \
               AND report_date BETWEEN $3 AND $4 \
             ORDER BY report_date, sales_channel_id, currency LIMIT 5001",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(from)
        .bind(to)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let attribution = sqlx::query_as::<_, DailyAttributionReportRow>(
            "SELECT sales_channel_id, report_date, attribution_model::text, model_version, \
                    is_direct, campaign_source, campaign_medium, campaign_name, \
                    attributed_orders, attributed_amount_minor, currency::text AS currency, \
                    refreshed_at \
             FROM analytics.daily_attribution_reports \
             WHERE merchant_account_id = $1 AND store_id = $2 \
               AND report_date BETWEEN $3 AND $4 \
             ORDER BY report_date, attribution_model, model_version, sales_channel_id, \
                      campaign_source, campaign_medium, campaign_name, currency LIMIT 5001",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(from)
        .bind(to)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        if behavior.len() > 5000 || commerce.len() > 5000 || attribution.len() > 5000 {
            return Err(ApplicationError::Conflict {
                code: "analytics_report_too_large",
                message: "the requested Analytics report exceeds the bounded response size",
            });
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(AnalyticsDailyReports {
            behavior: behavior
                .into_iter()
                .map(behavior_report)
                .collect::<Result<_, _>>()?,
            commerce: commerce
                .into_iter()
                .map(commerce_report)
                .collect::<Result<_, _>>()?,
            attribution: attribution
                .into_iter()
                .map(attribution_report)
                .collect::<Result<_, _>>()?,
        }))
    }
}

impl PostgresAnalyticsEventRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AnalyticsEventRepository for PostgresAnalyticsEventRepository {
    async fn resolve_collection_policy(
        &self,
        actor: &MachineActor,
        now: OffsetDateTime,
    ) -> Result<ResolvedAnalyticsPolicy, ApplicationError> {
        let sales_channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(&mut transaction, actor.merchant_account_id.as_uuid(), None).await?;
        let context_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM merchant.stores AS store \
             INNER JOIN merchant.sales_channels AS channel \
               ON channel.merchant_account_id = store.merchant_account_id \
              AND channel.store_id = store.id \
             WHERE store.merchant_account_id = $1 AND store.id = $2 \
               AND store.status = 'active' AND channel.id = $3 AND channel.status = 'active')",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !context_exists {
            return Err(ApplicationError::Forbidden);
        }
        let policy = load_current_policy(
            &mut transaction,
            actor.merchant_account_id.as_uuid(),
            actor.store_id,
            now,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(policy.map_or_else(
            || ResolvedAnalyticsPolicy {
                policy: AnalyticsPolicy::builtin(),
                policy_version: "builtin-v1".into(),
            },
            |item| ResolvedAnalyticsPolicy {
                policy: item.policy,
                policy_version: item.policy_version,
            },
        ))
    }

    async fn append_browser_events(
        &self,
        actor: &MachineActor,
        events: &[BrowserEvent],
        collection_policy_version: &str,
        advertising_exports_enabled: bool,
        received_at: OffsetDateTime,
        retention_expires_at: OffsetDateTime,
    ) -> Result<usize, ApplicationError> {
        let sales_channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(actor.merchant_account_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let context_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM merchant.stores AS store \
             INNER JOIN merchant.sales_channels AS channel \
               ON channel.merchant_account_id = store.merchant_account_id \
              AND channel.store_id = store.id \
             WHERE store.merchant_account_id = $1 AND store.id = $2 \
               AND store.status = 'active' AND channel.id = $3 AND channel.status = 'active')",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !context_exists {
            return Err(ApplicationError::Forbidden);
        }

        let mut stored = 0;
        for event in events {
            let attribution = attribution_columns(event.properties());
            let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
                "INSERT INTO analytics.behavior_events \
                 (id, event_id, merchant_account_id, store_id, sales_channel_id, event_name, \
                  schema_version, source, anonymous_id, session_id, analytics_storage_consent, \
                  advertising_storage_consent, advertising_export_eligible, consent_policy_version, \
                  collection_policy_version, properties, landing_path, referrer_domain, \
                  campaign_source, campaign_medium, campaign_name, cart_id, checkout_id, \
                  occurred_at, received_at, \
                  retention_expires_at) \
                 VALUES (uuidv7(),$1,$2,$3,$4,$5::analytics.browser_event_name,$6, \
                         'browser',$7,$8,true,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23) \
                 ON CONFLICT (merchant_account_id, store_id, event_id) DO NOTHING \
                 RETURNING id",
            )
            .bind(event.event_id())
            .bind(actor.merchant_account_id.as_uuid())
            .bind(actor.store_id.as_uuid())
            .bind(sales_channel_id.as_uuid())
            .bind(event.name().as_str())
            .bind(i16::try_from(event.schema_version()).map_err(conversion_error)?)
            .bind(event.anonymous_id())
            .bind(event.session_id())
            .bind(event.consent().advertising_storage())
            .bind(event.consent().advertising_storage() && advertising_exports_enabled)
            .bind(event.consent().policy_version())
            .bind(collection_policy_version)
            .bind(properties(event.properties()))
            .bind(attribution.landing_path)
            .bind(attribution.referrer_domain)
            .bind(attribution.campaign_source)
            .bind(attribution.campaign_medium)
            .bind(attribution.campaign_name)
            .bind(attribution.cart_id)
            .bind(attribution.checkout_id)
            .bind(event.occurred_at())
            .bind(received_at)
            .bind(retention_expires_at)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?;
            if let Some(behavior_event_id) = inserted {
                sqlx::query(
                    "INSERT INTO analytics.behavior_event_processing \
                     (id, merchant_account_id, store_id, available_at) VALUES ($1,$2,$3,$4)",
                )
                .bind(behavior_event_id)
                .bind(actor.merchant_account_id.as_uuid())
                .bind(actor.store_id.as_uuid())
                .bind(received_at)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
            }
            stored += usize::from(inserted.is_some());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(stored)
    }
}

#[derive(FromRow)]
struct PolicyRow {
    id: Uuid,
    store_id: Uuid,
    version: i32,
    behavior_collection_enabled: bool,
    advertising_exports_enabled: bool,
    identity_linking_enabled: bool,
    raw_event_retention_days: i16,
    created_by: Uuid,
    effective_at: OffsetDateTime,
    created_at: OffsetDateTime,
}

#[derive(FromRow)]
struct IdentityLinkRow {
    id: Uuid,
    store_id: Uuid,
    anonymous_id: Uuid,
    customer_id: Uuid,
    consent_policy_version: String,
    collection_policy_version: String,
    linked_at: OffsetDateTime,
    retention_expires_at: OffsetDateTime,
}

#[derive(FromRow)]
struct ErasureRequestRow {
    id: Uuid,
    store_id: Uuid,
    anonymous_id: Option<Uuid>,
    customer_id: Option<Uuid>,
    status: String,
    requested_by: Uuid,
    behavior_events_deleted: i64,
    attribution_results_deleted: i64,
    sessions_deleted: i64,
    identity_links_deleted: i64,
    requested_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
}

#[async_trait]
impl AnalyticsPolicyRepository for PostgresAnalyticsEventRepository {
    async fn get_store_policy(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        now: OffsetDateTime,
    ) -> Result<Option<StoreAnalyticsPolicy>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            Some(actor.user_id().as_uuid()),
        )
        .await?;
        let store_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM merchant.stores \
             WHERE merchant_account_id = $1 AND id = $2)",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !store_exists {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        }
        let policy = load_current_policy(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            store_id,
            now,
        )
        .await?
        .unwrap_or_else(|| builtin_policy(store_id));
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(policy))
    }

    async fn update_store_policy(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        policy: AnalyticsPolicy,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreAnalyticsPolicy, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            Some(actor.user_id().as_uuid()),
        )
        .await?;
        if let Some(policy_id) = reserve_policy(&mut transaction, actor, request).await? {
            let item = load_policy_by_id(
                &mut transaction,
                actor.merchant_account_id().as_uuid(),
                store_id,
                policy_id,
            )
            .await?
            .ok_or_else(invalid_policy_snapshot)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(item);
        }
        let store_status = sqlx::query_scalar::<_, String>(
            "SELECT status::text FROM merchant.stores \
             WHERE merchant_account_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| store_not_found(store_id))?;
        if store_status == "archived" {
            return Err(ApplicationError::Conflict {
                code: "store_not_writable",
                message: "an archived Store cannot accept Analytics Policy changes",
            });
        }
        let next_version: i32 = sqlx::query_scalar(
            "SELECT COALESCE(max(version), 0) + 1 \
             FROM analytics.store_policy_versions \
             WHERE merchant_account_id = $1 AND store_id = $2",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let policy_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO analytics.store_policy_versions \
             (id, merchant_account_id, store_id, version, behavior_collection_enabled, \
              advertising_exports_enabled, identity_linking_enabled, raw_event_retention_days, \
              created_by, effective_at, created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10)",
        )
        .bind(policy_id)
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(next_version)
        .bind(policy.behavior_collection_enabled())
        .bind(policy.advertising_exports_enabled())
        .bind(policy.identity_linking_enabled())
        .bind(i16::try_from(policy.raw_event_retention_days()).map_err(conversion_error)?)
        .bind(actor.user_id().as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query("SELECT * FROM analytics.apply_store_retention_policy($1,$2,$3)")
            .bind(actor.merchant_account_id().as_uuid())
            .bind(store_id.as_uuid())
            .bind(i32::from(policy.raw_event_retention_days()))
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        complete_policy(&mut transaction, actor, request, policy_id).await?;
        let item = load_policy_by_id(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            store_id,
            policy_id,
        )
        .await?
        .ok_or_else(invalid_policy_snapshot)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(item)
    }
}

#[async_trait]
impl AnalyticsPrivacyRepository for PostgresAnalyticsEventRepository {
    async fn link_customer_identity(
        &self,
        actor: &CustomerActor,
        anonymous_id: Uuid,
        consent: &ConsentSnapshot,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsIdentityLink, ApplicationError> {
        let machine = &actor.machine;
        let sales_channel_id = machine
            .sales_channel_id
            .ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(
            &mut transaction,
            machine.merchant_account_id.as_uuid(),
            Some(actor.user_id.as_uuid()),
        )
        .await?;
        if let Some(link_id) = reserve_identity_link(&mut transaction, actor, request).await? {
            let link = load_identity_link_by_id(
                &mut transaction,
                machine.merchant_account_id.as_uuid(),
                machine.store_id,
                link_id,
            )
            .await?
            .ok_or_else(invalid_identity_link_snapshot)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(link);
        }
        let customer_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT customer.id FROM sales.customers AS customer \
             INNER JOIN merchant.stores AS store \
               ON store.merchant_account_id = customer.merchant_account_id \
              AND store.id = customer.store_id \
             INNER JOIN merchant.sales_channels AS channel \
               ON channel.merchant_account_id = customer.merchant_account_id \
              AND channel.store_id = customer.store_id \
             WHERE customer.merchant_account_id = $1 AND customer.store_id = $2 \
               AND customer.user_id = $3 AND store.status = 'active' \
               AND channel.id = $4 AND channel.status = 'active'",
        )
        .bind(machine.merchant_account_id.as_uuid())
        .bind(machine.store_id.as_uuid())
        .bind(actor.user_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or(ApplicationError::NotFound {
            resource: "customer",
            id: actor.user_id.as_uuid().to_string(),
        })?;
        let resolved = load_current_policy(
            &mut transaction,
            machine.merchant_account_id.as_uuid(),
            machine.store_id,
            now,
        )
        .await?
        .unwrap_or_else(|| builtin_policy(machine.store_id));
        if !resolved.policy.identity_linking_enabled() {
            return Err(ApplicationError::Conflict {
                code: "analytics_identity_linking_disabled",
                message: "the effective Store Analytics Policy does not allow identity linking",
            });
        }
        let consent_evidence_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM analytics.behavior_events \
             WHERE merchant_account_id = $1 AND store_id = $2 AND anonymous_id = $3 \
               AND consent_policy_version = $4 AND analytics_storage_consent)",
        )
        .bind(machine.merchant_account_id.as_uuid())
        .bind(machine.store_id.as_uuid())
        .bind(anonymous_id)
        .bind(consent.policy_version())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !consent_evidence_exists {
            return Err(ApplicationError::NotFound {
                resource: "analytics_anonymous_identity",
                id: anonymous_id.to_string(),
            });
        }
        let link_id = Uuid::now_v7();
        let inserted = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO analytics.identity_links \
             (id, merchant_account_id, store_id, anonymous_id, customer_id, \
              consent_policy_version, collection_policy_version, linked_at, \
              retention_expires_at, created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$8) \
             ON CONFLICT (merchant_account_id, store_id, anonymous_id) DO NOTHING \
             RETURNING id",
        )
        .bind(link_id)
        .bind(machine.merchant_account_id.as_uuid())
        .bind(machine.store_id.as_uuid())
        .bind(anonymous_id)
        .bind(customer_id)
        .bind(consent.policy_version())
        .bind(&resolved.policy_version)
        .bind(now)
        .bind(now + time::Duration::days(i64::from(resolved.policy.raw_event_retention_days())))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let actual_id = if let Some(id) = inserted {
            id
        } else {
            let existing = load_identity_link_by_anonymous(
                &mut transaction,
                machine.merchant_account_id.as_uuid(),
                machine.store_id,
                anonymous_id,
            )
            .await?
            .ok_or_else(invalid_identity_link_snapshot)?;
            if existing.customer_id.as_uuid() != customer_id {
                return Err(ApplicationError::Conflict {
                    code: "analytics_identity_already_linked",
                    message: "the anonymous Analytics identity is already linked",
                });
            }
            existing.id
        };
        complete_identity_link(&mut transaction, actor, request, actual_id).await?;
        let link = load_identity_link_by_id(
            &mut transaction,
            machine.merchant_account_id.as_uuid(),
            machine.store_id,
            actual_id,
        )
        .await?
        .ok_or_else(invalid_identity_link_snapshot)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(link)
    }

    async fn request_erasure(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        selector: AnalyticsErasureSelector,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsErasureRequest, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            Some(actor.user_id().as_uuid()),
        )
        .await?;
        if let Some(request_id) = reserve_erasure(&mut transaction, actor, request).await? {
            let item = load_erasure_request(
                &mut transaction,
                actor.merchant_account_id().as_uuid(),
                store_id,
                request_id,
            )
            .await?
            .ok_or_else(invalid_erasure_snapshot)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(item);
        }
        let store_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM merchant.stores \
             WHERE merchant_account_id = $1 AND id = $2)",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !store_exists {
            return Err(store_not_found(store_id));
        }
        let (anonymous_id, customer_id) = match selector {
            AnalyticsErasureSelector::Anonymous(id) => (Some(id), None),
            AnalyticsErasureSelector::Customer(id) => (None, Some(id.as_uuid())),
        };
        let request_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO analytics.erasure_requests \
             (id, merchant_account_id, store_id, anonymous_id, customer_id, requested_by, \
              requested_at, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$7,$7)",
        )
        .bind(request_id)
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(anonymous_id)
        .bind(customer_id)
        .bind(actor.user_id().as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        complete_erasure(&mut transaction, actor, request, request_id).await?;
        let item = load_erasure_request(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            store_id,
            request_id,
        )
        .await?
        .ok_or_else(invalid_erasure_snapshot)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(item)
    }

    async fn get_erasure_request(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        request_id: Uuid,
    ) -> Result<Option<AnalyticsErasureRequest>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            Some(actor.user_id().as_uuid()),
        )
        .await?;
        let item = load_erasure_request(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            store_id,
            request_id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(item)
    }

    async fn process_erasure_requests(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsErasureBatchResult, ApplicationError> {
        let row: (i64, i64, i64, i64, i64) =
            sqlx::query_as("SELECT * FROM analytics.process_erasure_requests($1,$2)")
                .bind(i32::from(limit))
                .bind(now)
                .fetch_one(&self.pool)
                .await
                .map_err(database_error)?;
        Ok(AnalyticsErasureBatchResult {
            requests_completed: u64::try_from(row.0).map_err(conversion_error)?,
            behavior_events_deleted: u64::try_from(row.1).map_err(conversion_error)?,
            attribution_results_deleted: u64::try_from(row.2).map_err(conversion_error)?,
            sessions_deleted: u64::try_from(row.3).map_err(conversion_error)?,
            identity_links_deleted: u64::try_from(row.4).map_err(conversion_error)?,
        })
    }
}

#[derive(FromRow)]
struct SessionizationClaimRow {
    behavior_event_id: Uuid,
    merchant_account_id: Uuid,
    store_id: Uuid,
    sales_channel_id: Uuid,
    event_name: String,
    anonymous_id: Uuid,
    client_session_id: Uuid,
    occurred_at: OffsetDateTime,
    retention_expires_at: OffsetDateTime,
    active_engagement_milliseconds: Option<i32>,
    attempts: i32,
}

#[derive(FromRow)]
struct SessionRow {
    id: Uuid,
    started_at: OffsetDateTime,
    last_event_at: OffsetDateTime,
    event_count: i64,
    page_view_count: i64,
    product_view_count: i64,
    search_count: i64,
    cart_line_added_count: i64,
    checkout_started_count: i64,
    active_engagement_milliseconds: i64,
    retention_expires_at: OffsetDateTime,
}

#[async_trait]
impl AnalyticsSessionizationQueue for PostgresAnalyticsEventRepository {
    async fn claim_sessionization(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<AnalyticsSessionizationJob>, ApplicationError> {
        let rows = sqlx::query_as::<_, SessionizationClaimRow>(
            "SELECT behavior_event_id, merchant_account_id, store_id, sales_channel_id, \
                    event_name, anonymous_id, client_session_id, occurred_at, \
                    retention_expires_at, active_engagement_milliseconds, attempts \
             FROM analytics.claim_sessionization_events($1,$2,$3,$4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(AnalyticsSessionizationJob {
                    behavior_event_id: row.behavior_event_id,
                    merchant_account_id: row.merchant_account_id,
                    store_id: StoreId::from_uuid(row.store_id),
                    sales_channel_id: SalesChannelId::from_uuid(row.sales_channel_id),
                    event_name: BrowserEventName::parse(&row.event_name)
                        .ok_or_else(corrupt_sessionization_event)?,
                    anonymous_id: row.anonymous_id,
                    client_session_id: row.client_session_id,
                    occurred_at: row.occurred_at,
                    retention_expires_at: row.retention_expires_at,
                    active_engagement_milliseconds: row
                        .active_engagement_milliseconds
                        .map(u32::try_from)
                        .transpose()
                        .map_err(conversion_error)?,
                    attempts: u32::try_from(row.attempts).map_err(conversion_error)?,
                })
            })
            .collect()
    }

    async fn finish_sessionization(
        &self,
        worker_id: Uuid,
        job: &AnalyticsSessionizationJob,
        result: Result<SessionEventContribution, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(job.merchant_account_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        match result {
            Ok(contribution) => {
                apply_session_contribution(&mut transaction, job, contribution, now).await?;
                refresh_behavior_report(
                    &mut transaction,
                    job.merchant_account_id,
                    job.store_id,
                    job.sales_channel_id,
                    job.occurred_at.date(),
                    now,
                )
                .await?;
                let prior_date = (job.occurred_at - time::Duration::minutes(30)).date();
                if prior_date != job.occurred_at.date() {
                    refresh_behavior_report(
                        &mut transaction,
                        job.merchant_account_id,
                        job.store_id,
                        job.sales_channel_id,
                        prior_date,
                        now,
                    )
                    .await?;
                }
                let updated = sqlx::query(
                    "UPDATE analytics.behavior_event_processing \
                     SET processing_status = 'processed', processed_at = $5, \
                         locked_by = NULL, locked_at = NULL, last_error = NULL, updated_at = $5 \
                     WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
                       AND locked_by = $4 AND processing_status = 'processing'",
                )
                .bind(job.merchant_account_id)
                .bind(job.store_id.as_uuid())
                .bind(job.behavior_event_id)
                .bind(worker_id)
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
                if updated.rows_affected() != 1 {
                    return Err(stale_sessionization_lease());
                }
            }
            Err(error) => {
                let updated = sqlx::query(
                    "UPDATE analytics.behavior_event_processing \
                     SET processing_status = CASE WHEN attempts >= 8 \
                             THEN 'dead_letter'::integration.queue_status \
                             ELSE 'pending'::integration.queue_status END, \
                         available_at = CASE WHEN attempts >= 8 THEN available_at \
                             ELSE $5 + make_interval(secs => least(power(2, \
                                 greatest(attempts - 1, 0))::integer, 3600)) END, \
                         locked_by = NULL, locked_at = NULL, last_error = $6, updated_at = $5 \
                     WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
                       AND locked_by = $4 AND processing_status = 'processing'",
                )
                .bind(job.merchant_account_id)
                .bind(job.store_id.as_uuid())
                .bind(job.behavior_event_id)
                .bind(worker_id)
                .bind(now)
                .bind(bounded_error(error))
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
                if updated.rows_affected() != 1 {
                    return Err(stale_sessionization_lease());
                }
            }
        }
        transaction.commit().await.map_err(database_error)
    }

    async fn purge_expired_data(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsRetentionPurgeResult, ApplicationError> {
        let (
            behavior_events_deleted,
            attribution_results_deleted,
            sessions_deleted,
            identity_links_deleted,
        ): (i64, i64, i64, i64) =
            sqlx::query_as("SELECT * FROM analytics.purge_expired_data($1,$2)")
                .bind(i32::from(limit))
                .bind(now)
                .fetch_one(&self.pool)
                .await
                .map_err(database_error)?;
        Ok(AnalyticsRetentionPurgeResult {
            behavior_events_deleted: u64::try_from(behavior_events_deleted)
                .map_err(conversion_error)?,
            attribution_results_deleted: u64::try_from(attribution_results_deleted)
                .map_err(conversion_error)?,
            sessions_deleted: u64::try_from(sessions_deleted).map_err(conversion_error)?,
            identity_links_deleted: u64::try_from(identity_links_deleted)
                .map_err(conversion_error)?,
        })
    }
}

#[async_trait]
impl AnalyticsCommerceFactQueue for PostgresAnalyticsEventRepository {
    async fn claim_commerce_facts(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<AnalyticsCommerceFactJob>, ApplicationError> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Value, i32, OffsetDateTime)>(
            "SELECT id, merchant_account_id, store_id, event_type, payload, attempts, occurred_at \
             FROM analytics.claim_commerce_fact_events($1,$2,$3,$4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit.clamp(1, 100)))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(AnalyticsCommerceFactJob {
                    id: row.0,
                    merchant_account_id: row.1,
                    store_id: StoreId::from_uuid(row.2),
                    event_type: row.3,
                    payload: row.4,
                    attempts: u32::try_from(row.5).map_err(conversion_error)?,
                    occurred_at: row.6,
                })
            })
            .collect()
    }

    async fn ingest_commerce_fact(
        &self,
        job: &AnalyticsCommerceFactJob,
        ingested_at: OffsetDateTime,
        attribution_model_version: u16,
        attribution_available_at: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(&mut transaction, job.merchant_account_id, None).await?;
        let inserted = match job.event_type.as_str() {
            "analytics.order.created" => {
                let order_id = fact_payload_uuid(&job.payload, "order_id")?;
                sqlx::query(
                    "INSERT INTO analytics.commerce_facts \
                     (id, merchant_account_id, store_id, sales_channel_id, fact_name, \
                      schema_version, order_id, customer_id, amount_minor, currency, \
                      occurred_at, ingested_at) \
                     SELECT $1, orders.merchant_account_id, orders.store_id, \
                            orders.sales_channel_id, 'order_created', 1, orders.id, \
                            orders.customer_id, orders.total_amount_minor, orders.currency, $5, $6 \
                       FROM sales.orders AS orders \
                      WHERE orders.merchant_account_id = $2 AND orders.store_id = $3 \
                        AND orders.id = $4 ON CONFLICT (id) DO NOTHING",
                )
                .bind(job.id)
                .bind(job.merchant_account_id)
                .bind(job.store_id.as_uuid())
                .bind(order_id)
                .bind(job.occurred_at)
                .bind(ingested_at)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?
            }
            "analytics.payment.captured" => {
                let attempt_id = fact_payload_uuid(&job.payload, "payment_attempt_id")?;
                sqlx::query(
                    "INSERT INTO analytics.commerce_facts \
                     (id, merchant_account_id, store_id, sales_channel_id, fact_name, \
                      schema_version, order_id, customer_id, payment_attempt_id, amount_minor, \
                      currency, occurred_at, ingested_at) \
                     SELECT $1, attempt.merchant_account_id, attempt.store_id, \
                            orders.sales_channel_id, 'payment_captured', 1, orders.id, \
                            orders.customer_id, attempt.id, attempt.amount_minor, attempt.currency, $5, $6 \
                       FROM payments.payment_attempts AS attempt \
                       INNER JOIN sales.orders AS orders \
                         ON orders.merchant_account_id = attempt.merchant_account_id \
                        AND orders.store_id = attempt.store_id AND orders.id = attempt.order_id \
                      WHERE attempt.merchant_account_id = $2 AND attempt.store_id = $3 \
                        AND attempt.id = $4 AND attempt.status = 'captured' \
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(job.id)
                .bind(job.merchant_account_id)
                .bind(job.store_id.as_uuid())
                .bind(attempt_id)
                .bind(job.occurred_at)
                .bind(ingested_at)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?
            }
            "analytics.refund.succeeded" => {
                let refund_id = fact_payload_uuid(&job.payload, "refund_id")?;
                sqlx::query(
                    "INSERT INTO analytics.commerce_facts \
                     (id, merchant_account_id, store_id, sales_channel_id, fact_name, \
                      schema_version, order_id, customer_id, payment_attempt_id, refund_id, \
                      amount_minor, currency, occurred_at, ingested_at) \
                     SELECT $1, refund.merchant_account_id, refund.store_id, \
                            orders.sales_channel_id, 'refund_succeeded', 1, orders.id, \
                            orders.customer_id, attempt.id, refund.id, refund.amount_minor, \
                            refund.currency, $5, $6 \
                       FROM payments.refunds AS refund \
                       INNER JOIN payments.payment_attempts AS attempt \
                         ON attempt.merchant_account_id = refund.merchant_account_id \
                        AND attempt.store_id = refund.store_id \
                        AND attempt.id = refund.payment_attempt_id \
                       INNER JOIN sales.orders AS orders \
                         ON orders.merchant_account_id = attempt.merchant_account_id \
                        AND orders.store_id = attempt.store_id AND orders.id = attempt.order_id \
                      WHERE refund.merchant_account_id = $2 AND refund.store_id = $3 \
                        AND refund.id = $4 AND refund.status = 'succeeded' \
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(job.id)
                .bind(job.merchant_account_id)
                .bind(job.store_id.as_uuid())
                .bind(refund_id)
                .bind(job.occurred_at)
                .bind(ingested_at)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?
            }
            "analytics.fulfillment.shipped" => {
                let fulfillment_id = fact_payload_uuid(&job.payload, "fulfillment_id")?;
                sqlx::query(
                    "INSERT INTO analytics.commerce_facts \
                     (id, merchant_account_id, store_id, sales_channel_id, fact_name, \
                      schema_version, order_id, customer_id, fulfillment_id, occurred_at, ingested_at) \
                     SELECT $1, fulfillment.merchant_account_id, fulfillment.store_id, \
                            orders.sales_channel_id, 'fulfillment_shipped', 1, orders.id, \
                            orders.customer_id, fulfillment.id, $5, $6 \
                       FROM fulfillment.fulfillments AS fulfillment \
                       INNER JOIN sales.orders AS orders \
                         ON orders.merchant_account_id = fulfillment.merchant_account_id \
                        AND orders.store_id = fulfillment.store_id \
                        AND orders.id = fulfillment.order_id \
                      WHERE fulfillment.merchant_account_id = $2 AND fulfillment.store_id = $3 \
                        AND fulfillment.id = $4 AND fulfillment.shipped_at IS NOT NULL \
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(job.id)
                .bind(job.merchant_account_id)
                .bind(job.store_id.as_uuid())
                .bind(fulfillment_id)
                .bind(job.occurred_at)
                .bind(ingested_at)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?
            }
            "analytics.return.completed" => {
                let return_id = fact_payload_uuid(&job.payload, "return_id")?;
                sqlx::query(
                    "INSERT INTO analytics.commerce_facts \
                     (id, merchant_account_id, store_id, sales_channel_id, fact_name, \
                      schema_version, order_id, customer_id, return_id, occurred_at, ingested_at) \
                     SELECT $1, returned.merchant_account_id, returned.store_id, \
                            orders.sales_channel_id, 'return_completed', 1, orders.id, \
                            orders.customer_id, returned.id, $5, $6 \
                       FROM fulfillment.returns AS returned \
                       INNER JOIN sales.orders AS orders \
                         ON orders.merchant_account_id = returned.merchant_account_id \
                        AND orders.store_id = returned.store_id AND orders.id = returned.order_id \
                      WHERE returned.merchant_account_id = $2 AND returned.store_id = $3 \
                        AND returned.id = $4 AND returned.status = 'completed' \
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(job.id)
                .bind(job.merchant_account_id)
                .bind(job.store_id.as_uuid())
                .bind(return_id)
                .bind(job.occurred_at)
                .bind(ingested_at)
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?
            }
            _ => return Err(corrupt_commerce_fact_event()),
        };
        if inserted.rows_affected() == 0 {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM analytics.commerce_facts \
                 WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3)",
            )
            .bind(job.merchant_account_id)
            .bind(job.store_id.as_uuid())
            .bind(job.id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            if !exists {
                return Err(corrupt_commerce_fact_event());
            }
        }
        if job.event_type == "analytics.order.created" {
            sqlx::query(
                "INSERT INTO analytics.attribution_jobs \
                 (commerce_fact_id, merchant_account_id, store_id, model_version, available_at) \
                 VALUES ($1,$2,$3,$4,$5) \
                 ON CONFLICT (merchant_account_id, store_id, commerce_fact_id, model_version) \
                 DO NOTHING",
            )
            .bind(job.id)
            .bind(job.merchant_account_id)
            .bind(job.store_id.as_uuid())
            .bind(i16::try_from(attribution_model_version).map_err(conversion_error)?)
            .bind(attribution_available_at)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        refresh_commerce_report(
            &mut transaction,
            job.merchant_account_id,
            job.store_id,
            job.occurred_at.date(),
            ingested_at,
        )
        .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn finish_commerce_fact(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (succeeded, failure) = match result {
            Ok(()) => (true, String::new()),
            Err(error) => (false, bounded_error(error)),
        };
        let finished: Option<bool> =
            sqlx::query_scalar("SELECT integration.finish_outbox_event($1,$2,$3,$4,$5,$6)")
                .bind(job_id)
                .bind(worker_id)
                .bind(succeeded)
                .bind(failure)
                .bind(8_i32)
                .bind(now)
                .fetch_one(&self.pool)
                .await
                .map_err(database_error)?;
        if finished == Some(true) {
            Ok(())
        } else {
            Err(ApplicationError::Conflict {
                code: "analytics_commerce_fact_lease_lost",
                message: "the Analytics commerce fact lease is no longer owned by this worker",
            })
        }
    }
}

#[async_trait]
impl AnalyticsAttributionQueue for PostgresAnalyticsEventRepository {
    async fn claim_attribution(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<AnalyticsAttributionJob>, ApplicationError> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, i16, i32)>(
            "SELECT commerce_fact_id, merchant_account_id, store_id, model_version, attempts \
             FROM analytics.claim_attribution_jobs($1,$2,$3,$4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit.clamp(1, 100)))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(AnalyticsAttributionJob {
                    commerce_fact_id: row.0,
                    merchant_account_id: row.1,
                    store_id: StoreId::from_uuid(row.2),
                    model_version: u16::try_from(row.3).map_err(conversion_error)?,
                    attempts: u32::try_from(row.4).map_err(conversion_error)?,
                })
            })
            .collect()
    }

    async fn attribute_order(
        &self,
        job: &AnalyticsAttributionJob,
        attributed_at: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(&mut transaction, job.merchant_account_id, None).await?;
        let affected = sqlx::query(
            "WITH order_context AS ( \
                 SELECT fact.id AS commerce_fact_id, fact.merchant_account_id, fact.store_id, \
                        fact.sales_channel_id, fact.order_id, fact.customer_id, fact.occurred_at, \
                        orders.checkout_id, checkout.cart_id \
                   FROM analytics.commerce_facts AS fact \
                   INNER JOIN sales.orders AS orders \
                     ON orders.merchant_account_id = fact.merchant_account_id \
                    AND orders.store_id = fact.store_id AND orders.id = fact.order_id \
                   INNER JOIN sales.checkouts AS checkout \
                     ON checkout.merchant_account_id = orders.merchant_account_id \
                    AND checkout.store_id = orders.store_id AND checkout.id = orders.checkout_id \
                  WHERE fact.merchant_account_id = $1 AND fact.store_id = $2 AND fact.id = $3 \
                    AND fact.fact_name = 'order_created' \
             ), anchor AS ( \
                 SELECT event.anonymous_id, event.session_id, event.occurred_at \
                   FROM analytics.behavior_events AS event \
                   INNER JOIN order_context AS context \
                     ON context.merchant_account_id = event.merchant_account_id \
                    AND context.store_id = event.store_id \
                    AND context.sales_channel_id = event.sales_channel_id \
                  WHERE event.event_name = 'checkout_started' \
                    AND (event.checkout_id = context.checkout_id OR event.cart_id = context.cart_id) \
                    AND event.occurred_at <= context.occurred_at + INTERVAL '5 minutes' \
                  ORDER BY (event.checkout_id = context.checkout_id) DESC, \
                           event.occurred_at DESC, event.id DESC LIMIT 1 \
             ), touches AS ( \
                 SELECT event.*, \
                        row_number() OVER (ORDER BY event.occurred_at, event.id) AS first_rank, \
                        row_number() OVER (ORDER BY event.occurred_at DESC, event.id DESC) AS last_rank \
                   FROM analytics.behavior_events AS event \
                   INNER JOIN order_context AS context \
                     ON context.merchant_account_id = event.merchant_account_id \
                    AND context.store_id = event.store_id \
                    AND context.sales_channel_id = event.sales_channel_id \
                   INNER JOIN anchor \
                     ON anchor.anonymous_id = event.anonymous_id \
                    AND anchor.session_id = event.session_id \
                  WHERE event.event_name = 'page_viewed' \
                    AND event.occurred_at <= anchor.occurred_at \
             ), watermark AS ( \
                 SELECT max(event.received_at) AS received_at \
                   FROM analytics.behavior_events AS event \
                   INNER JOIN order_context AS context \
                     ON context.merchant_account_id = event.merchant_account_id \
                    AND context.store_id = event.store_id \
                    AND context.sales_channel_id = event.sales_channel_id \
                   INNER JOIN anchor \
                     ON anchor.anonymous_id = event.anonymous_id \
                    AND anchor.session_id = event.session_id \
                  WHERE event.occurred_at <= context.occurred_at + INTERVAL '5 minutes' \
             ), models AS ( \
                 SELECT 'first_touch'::analytics.attribution_model AS attribution_model, \
                        touch.* FROM touches AS touch WHERE touch.first_rank = 1 \
                 UNION ALL \
                 SELECT 'last_touch'::analytics.attribution_model AS attribution_model, \
                        touch.* FROM touches AS touch WHERE touch.last_rank = 1 \
             ), requested_models AS ( \
                 SELECT requested.model_name, model.* \
                   FROM (VALUES \
                       ('first_touch'::analytics.attribution_model), \
                       ('last_touch'::analytics.attribution_model) \
                   ) AS requested(model_name) \
                   LEFT JOIN models AS model \
                     ON model.attribution_model = requested.model_name \
             ) \
             INSERT INTO analytics.attribution_results \
             (id, merchant_account_id, store_id, sales_channel_id, commerce_fact_id, order_id, \
              customer_id, checkout_id, cart_id, attribution_model, model_version, is_direct, \
              touch_event_id, anonymous_id, session_id, landing_path, referrer_domain, \
              campaign_source, campaign_medium, campaign_name, advertising_storage_consent, \
              consent_policy_version, collection_policy_version, advertising_export_eligible, \
              touch_occurred_at, input_event_watermark, attributed_at) \
             SELECT uuidv7(), context.merchant_account_id, context.store_id, \
                    context.sales_channel_id, context.commerce_fact_id, context.order_id, \
                    context.customer_id, context.checkout_id, context.cart_id, \
                    requested.model_name, $4, requested.id IS NULL, requested.id, \
                    requested.anonymous_id, requested.session_id, requested.landing_path, \
                    requested.referrer_domain, requested.campaign_source, \
                    requested.campaign_medium, requested.campaign_name, \
                    requested.advertising_storage_consent, requested.consent_policy_version, \
                    requested.collection_policy_version, \
                    COALESCE(requested.advertising_export_eligible, false), \
                    requested.occurred_at, watermark.received_at, $5 \
               FROM order_context AS context CROSS JOIN requested_models AS requested \
               LEFT JOIN watermark ON true \
             ON CONFLICT (merchant_account_id, store_id, commerce_fact_id, \
                          attribution_model, model_version) DO UPDATE \
                 SET is_direct = EXCLUDED.is_direct, touch_event_id = EXCLUDED.touch_event_id, \
                     anonymous_id = EXCLUDED.anonymous_id, session_id = EXCLUDED.session_id, \
                     landing_path = EXCLUDED.landing_path, \
                     referrer_domain = EXCLUDED.referrer_domain, \
                     campaign_source = EXCLUDED.campaign_source, \
                     campaign_medium = EXCLUDED.campaign_medium, \
                     campaign_name = EXCLUDED.campaign_name, \
                     advertising_storage_consent = EXCLUDED.advertising_storage_consent, \
                     consent_policy_version = EXCLUDED.consent_policy_version, \
                     collection_policy_version = EXCLUDED.collection_policy_version, \
                     advertising_export_eligible = EXCLUDED.advertising_export_eligible, \
                     touch_occurred_at = EXCLUDED.touch_occurred_at, \
                     input_event_watermark = EXCLUDED.input_event_watermark, \
                     attributed_at = EXCLUDED.attributed_at",
        )
        .bind(job.merchant_account_id)
        .bind(job.store_id.as_uuid())
        .bind(job.commerce_fact_id)
        .bind(i16::try_from(job.model_version).map_err(conversion_error)?)
        .bind(attributed_at)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if affected.rows_affected() != 2 {
            return Err(corrupt_attribution_job());
        }
        refresh_attribution_report(
            &mut transaction,
            job.merchant_account_id,
            job.store_id,
            job.commerce_fact_id,
            attributed_at,
        )
        .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn finish_attribution(
        &self,
        worker_id: Uuid,
        job: &AnalyticsAttributionJob,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (status, processed_at, error, available_at) = match result {
            Ok(()) => ("processed", Some(now), None, now),
            Err(error) if job.attempts >= 8 => {
                ("dead_letter", None, Some(bounded_error(error)), now)
            }
            Err(error) => {
                let delay_seconds = 1_i64 << job.attempts.min(8);
                (
                    "pending",
                    None,
                    Some(bounded_error(error)),
                    now + time::Duration::seconds(delay_seconds),
                )
            }
        };
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_merchant_context(&mut transaction, job.merchant_account_id, None).await?;
        let finished = sqlx::query(
            "UPDATE analytics.attribution_jobs SET processing_status = $6::integration.queue_status, \
                    available_at = $7, locked_by = NULL, locked_at = NULL, processed_at = $8, \
                    last_error = $9, updated_at = $10 \
              WHERE merchant_account_id = $1 AND store_id = $2 AND commerce_fact_id = $3 \
                AND model_version = $4 AND processing_status = 'processing' AND locked_by = $5",
        )
        .bind(job.merchant_account_id)
        .bind(job.store_id.as_uuid())
        .bind(job.commerce_fact_id)
        .bind(i16::try_from(job.model_version).map_err(conversion_error)?)
        .bind(worker_id)
        .bind(status)
        .bind(available_at)
        .bind(processed_at)
        .bind(error)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if finished.rows_affected() != 1 {
            return Err(ApplicationError::Conflict {
                code: "analytics_attribution_lease_lost",
                message: "the Analytics attribution lease is no longer owned by this worker",
            });
        }
        transaction.commit().await.map_err(database_error)
    }
}

async fn apply_session_contribution(
    transaction: &mut Transaction<'_, Postgres>,
    job: &AnalyticsSessionizationJob,
    contribution: SessionEventContribution,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let lock_key = format!(
        "{}:{}:{}:{}:{}",
        job.merchant_account_id,
        job.store_id.as_uuid(),
        job.sales_channel_id.as_uuid(),
        job.anonymous_id,
        job.client_session_id
    );
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    let candidates = sqlx::query_as::<_, SessionRow>(
        "SELECT id, started_at, last_event_at, event_count, page_view_count, \
                product_view_count, search_count, cart_line_added_count, \
                checkout_started_count, active_engagement_milliseconds, retention_expires_at \
         FROM analytics.sessions \
         WHERE merchant_account_id = $1 AND store_id = $2 AND sales_channel_id = $3 \
           AND anonymous_id = $4 AND client_session_id = $5 \
           AND started_at <= $6 + make_interval(mins => $7) \
           AND last_event_at >= $6 - make_interval(mins => $7) \
         ORDER BY started_at, id FOR UPDATE",
    )
    .bind(job.merchant_account_id)
    .bind(job.store_id.as_uuid())
    .bind(job.sales_channel_id.as_uuid())
    .bind(job.anonymous_id)
    .bind(job.client_session_id)
    .bind(job.occurred_at)
    .bind(i32::try_from(SESSION_INACTIVITY_MINUTES).map_err(conversion_error)?)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    if candidates.is_empty() {
        insert_session(transaction, job, contribution, now).await
    } else {
        merge_sessions(transaction, job, candidates, contribution, now).await
    }
}

async fn insert_session(
    transaction: &mut Transaction<'_, Postgres>,
    job: &AnalyticsSessionizationJob,
    contribution: SessionEventContribution,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO analytics.sessions \
         (id, merchant_account_id, store_id, sales_channel_id, anonymous_id, client_session_id, \
          started_at, last_event_at, event_count, page_view_count, product_view_count, \
          search_count, cart_line_added_count, checkout_started_count, \
          active_engagement_milliseconds, retention_expires_at, created_at, updated_at) \
         VALUES (uuidv7(),$1,$2,$3,$4,$5,$6,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$15)",
    )
    .bind(job.merchant_account_id)
    .bind(job.store_id.as_uuid())
    .bind(job.sales_channel_id.as_uuid())
    .bind(job.anonymous_id)
    .bind(job.client_session_id)
    .bind(job.occurred_at)
    .bind(to_i64(contribution.event_count)?)
    .bind(to_i64(contribution.page_view_count)?)
    .bind(to_i64(contribution.product_view_count)?)
    .bind(to_i64(contribution.search_count)?)
    .bind(to_i64(contribution.cart_line_added_count)?)
    .bind(to_i64(contribution.checkout_started_count)?)
    .bind(to_i64(contribution.active_engagement_milliseconds)?)
    .bind(job.retention_expires_at)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn merge_sessions(
    transaction: &mut Transaction<'_, Postgres>,
    job: &AnalyticsSessionizationJob,
    candidates: Vec<SessionRow>,
    contribution: SessionEventContribution,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let target_id = candidates[0].id;
    let started_at = candidates
        .iter()
        .map(|row| row.started_at)
        .chain([job.occurred_at])
        .min()
        .ok_or_else(corrupt_sessionization_event)?;
    let last_event_at = candidates
        .iter()
        .map(|row| row.last_event_at)
        .chain([job.occurred_at])
        .max()
        .ok_or_else(corrupt_sessionization_event)?;
    let retention_expires_at = candidates
        .iter()
        .map(|row| row.retention_expires_at)
        .chain([job.retention_expires_at])
        .max()
        .ok_or_else(corrupt_sessionization_event)?;
    let event_count = sum(&candidates, |row| row.event_count, contribution.event_count)?;
    let page_view_count = sum(
        &candidates,
        |row| row.page_view_count,
        contribution.page_view_count,
    )?;
    let product_view_count = sum(
        &candidates,
        |row| row.product_view_count,
        contribution.product_view_count,
    )?;
    let search_count = sum(
        &candidates,
        |row| row.search_count,
        contribution.search_count,
    )?;
    let cart_line_added_count = sum(
        &candidates,
        |row| row.cart_line_added_count,
        contribution.cart_line_added_count,
    )?;
    let checkout_started_count = sum(
        &candidates,
        |row| row.checkout_started_count,
        contribution.checkout_started_count,
    )?;
    let existing_engagement = candidates.iter().try_fold(0_u64, |total, row| {
        let value = u64::try_from(row.active_engagement_milliseconds).map_err(conversion_error)?;
        Ok::<_, ApplicationError>(total.saturating_add(value))
    })?;
    let active_engagement_milliseconds = capped_session_engagement(
        existing_engagement,
        contribution.active_engagement_milliseconds,
    )?;
    let merged_ids = candidates
        .iter()
        .skip(1)
        .map(|row| row.id)
        .collect::<Vec<_>>();
    if !merged_ids.is_empty() {
        sqlx::query(
            "DELETE FROM analytics.sessions \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = ANY($3)",
        )
        .bind(job.merchant_account_id)
        .bind(job.store_id.as_uuid())
        .bind(&merged_ids)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    sqlx::query(
        "UPDATE analytics.sessions \
         SET started_at = $4, last_event_at = $5, event_count = $6, page_view_count = $7, \
             product_view_count = $8, search_count = $9, cart_line_added_count = $10, \
             checkout_started_count = $11, active_engagement_milliseconds = $12, \
             retention_expires_at = $13, updated_at = $14 \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(job.merchant_account_id)
    .bind(job.store_id.as_uuid())
    .bind(target_id)
    .bind(started_at)
    .bind(last_event_at)
    .bind(event_count)
    .bind(page_view_count)
    .bind(product_view_count)
    .bind(search_count)
    .bind(cart_line_added_count)
    .bind(checkout_started_count)
    .bind(to_i64(active_engagement_milliseconds)?)
    .bind(retention_expires_at)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

fn sum(
    rows: &[SessionRow],
    value: impl Fn(&SessionRow) -> i64,
    addition: u64,
) -> Result<i64, ApplicationError> {
    let total = rows.iter().try_fold(addition, |total, row| {
        let value = u64::try_from(value(row)).map_err(conversion_error)?;
        total.checked_add(value).ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!("Analytics Session count overflow"))
        })
    })?;
    to_i64(total)
}

fn to_i64(value: u64) -> Result<i64, ApplicationError> {
    i64::try_from(value).map_err(conversion_error)
}

fn bounded_error(error: String) -> String {
    error.chars().take(2000).collect()
}

fn corrupt_sessionization_event() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "the Analytics sessionization queue contains an invalid event"
    ))
}

fn corrupt_attribution_job() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "the Analytics attribution queue references an invalid Order fact"
    ))
}

fn stale_sessionization_lease() -> ApplicationError {
    ApplicationError::Conflict {
        code: "analytics_sessionization_lease_lost",
        message: "the Analytics Sessionization lease is no longer owned by this Worker",
    }
}

async fn set_merchant_context(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
        .bind(merchant_account_id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    if let Some(user_id) = user_id {
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(user_id.to_string())
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
    }
    Ok(())
}

async fn load_identity_link_by_id(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    link_id: Uuid,
) -> Result<Option<AnalyticsIdentityLink>, ApplicationError> {
    load_identity_link(transaction, merchant_account_id, store_id, "id", link_id).await
}

async fn load_identity_link_by_anonymous(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    anonymous_id: Uuid,
) -> Result<Option<AnalyticsIdentityLink>, ApplicationError> {
    load_identity_link(
        transaction,
        merchant_account_id,
        store_id,
        "anonymous_id",
        anonymous_id,
    )
    .await
}

async fn load_identity_link(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    discriminator: &'static str,
    value: Uuid,
) -> Result<Option<AnalyticsIdentityLink>, ApplicationError> {
    let query = match discriminator {
        "id" => {
            "SELECT id, store_id, anonymous_id, customer_id, consent_policy_version, \
                    collection_policy_version, linked_at, retention_expires_at \
             FROM analytics.identity_links \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3"
        }
        "anonymous_id" => {
            "SELECT id, store_id, anonymous_id, customer_id, consent_policy_version, \
                    collection_policy_version, linked_at, retention_expires_at \
             FROM analytics.identity_links \
             WHERE merchant_account_id = $1 AND store_id = $2 AND anonymous_id = $3"
        }
        _ => return Err(invalid_identity_link_snapshot()),
    };
    let row = sqlx::query_as::<_, IdentityLinkRow>(query)
        .bind(merchant_account_id)
        .bind(store_id.as_uuid())
        .bind(value)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(row.map(identity_link_item))
}

fn identity_link_item(row: IdentityLinkRow) -> AnalyticsIdentityLink {
    AnalyticsIdentityLink {
        id: row.id,
        store_id: StoreId::from_uuid(row.store_id),
        anonymous_id: row.anonymous_id,
        customer_id: CustomerId::from_uuid(row.customer_id),
        consent_policy_version: row.consent_policy_version,
        collection_policy_version: row.collection_policy_version,
        linked_at: row.linked_at,
        retention_expires_at: row.retention_expires_at,
    }
}

async fn load_erasure_request(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    request_id: Uuid,
) -> Result<Option<AnalyticsErasureRequest>, ApplicationError> {
    let row = sqlx::query_as::<_, ErasureRequestRow>(
        "SELECT id, store_id, anonymous_id, customer_id, status::text, requested_by, \
                behavior_events_deleted, attribution_results_deleted, sessions_deleted, identity_links_deleted, \
                requested_at, completed_at \
         FROM analytics.erasure_requests \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(request_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(erasure_request_item).transpose()
}

fn erasure_request_item(
    row: ErasureRequestRow,
) -> Result<AnalyticsErasureRequest, ApplicationError> {
    let selector = match (row.anonymous_id, row.customer_id) {
        (Some(id), None) => AnalyticsErasureSelector::Anonymous(id),
        (None, Some(id)) => AnalyticsErasureSelector::Customer(CustomerId::from_uuid(id)),
        _ => return Err(invalid_erasure_snapshot()),
    };
    let status = match row.status.as_str() {
        "pending" => AnalyticsErasureStatus::Pending,
        "completed" => AnalyticsErasureStatus::Completed,
        _ => return Err(invalid_erasure_snapshot()),
    };
    Ok(AnalyticsErasureRequest {
        id: row.id,
        store_id: StoreId::from_uuid(row.store_id),
        selector,
        status,
        requested_by: UserId::from_uuid(row.requested_by),
        behavior_events_deleted: u64::try_from(row.behavior_events_deleted)
            .map_err(conversion_error)?,
        attribution_results_deleted: u64::try_from(row.attribution_results_deleted)
            .map_err(conversion_error)?,
        sessions_deleted: u64::try_from(row.sessions_deleted).map_err(conversion_error)?,
        identity_links_deleted: u64::try_from(row.identity_links_deleted)
            .map_err(conversion_error)?,
        requested_at: row.requested_at,
        completed_at: row.completed_at,
    })
}

async fn reserve_identity_link(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &CustomerActor,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    reserve_snapshot_id(
        transaction,
        &IdempotencyScope::User(actor.user_id.as_uuid()),
        LINK_IDENTITY_OPERATION,
        request,
    )
    .await
}

async fn complete_identity_link(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &CustomerActor,
    request: &IdempotencyRequest,
    link_id: Uuid,
) -> Result<(), ApplicationError> {
    complete_snapshot_id(
        transaction,
        &IdempotencyScope::User(actor.user_id.as_uuid()),
        LINK_IDENTITY_OPERATION,
        request,
        201,
        link_id,
    )
    .await
}

async fn reserve_erasure(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    reserve_snapshot_id(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        REQUEST_ERASURE_OPERATION,
        request,
    )
    .await
}

async fn complete_erasure(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    request: &IdempotencyRequest,
    request_id: Uuid,
) -> Result<(), ApplicationError> {
    complete_snapshot_id(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        REQUEST_ERASURE_OPERATION,
        request,
        202,
        request_id,
    )
    .await
}

async fn reserve_snapshot_id(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(snapshot) = idempotency::reserve(transaction, scope, operation, request).await? else {
        return Ok(None);
    };
    snapshot
        .pointer("/data/id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(Some)
        .ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "completed Analytics privacy idempotency record is invalid"
            ))
        })
}

async fn complete_snapshot_id(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
    status: i16,
    id: Uuid,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        scope,
        operation,
        request,
        status,
        json!({ "data": { "id": id } }),
    )
    .await
}

async fn load_current_policy(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    now: OffsetDateTime,
) -> Result<Option<StoreAnalyticsPolicy>, ApplicationError> {
    let row = sqlx::query_as::<_, PolicyRow>(
        "SELECT id, store_id, version, behavior_collection_enabled, \
                advertising_exports_enabled, identity_linking_enabled, \
                raw_event_retention_days, created_by, effective_at, created_at \
         FROM analytics.store_policy_versions \
         WHERE merchant_account_id = $1 AND store_id = $2 AND effective_at <= $3 \
         ORDER BY effective_at DESC, version DESC LIMIT 1",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(now)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(policy_item).transpose()
}

async fn load_policy_by_id(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    policy_id: Uuid,
) -> Result<Option<StoreAnalyticsPolicy>, ApplicationError> {
    let row = sqlx::query_as::<_, PolicyRow>(
        "SELECT id, store_id, version, behavior_collection_enabled, \
                advertising_exports_enabled, identity_linking_enabled, \
                raw_event_retention_days, created_by, effective_at, created_at \
         FROM analytics.store_policy_versions \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(policy_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(policy_item).transpose()
}

fn policy_item(row: PolicyRow) -> Result<StoreAnalyticsPolicy, ApplicationError> {
    let retention_days = u16::try_from(row.raw_event_retention_days).map_err(conversion_error)?;
    Ok(StoreAnalyticsPolicy {
        id: Some(row.id),
        store_id: StoreId::from_uuid(row.store_id),
        policy: AnalyticsPolicy::new(
            row.behavior_collection_enabled,
            row.advertising_exports_enabled,
            row.identity_linking_enabled,
            retention_days,
        )?,
        policy_version: format!("store-v{}", row.version),
        created_by: Some(UserId::from_uuid(row.created_by)),
        effective_at: Some(row.effective_at),
        created_at: Some(row.created_at),
    })
}

fn builtin_policy(store_id: StoreId) -> StoreAnalyticsPolicy {
    StoreAnalyticsPolicy {
        id: None,
        store_id,
        policy: AnalyticsPolicy::builtin(),
        policy_version: "builtin-v1".into(),
        created_by: None,
        effective_at: None,
        created_at: None,
    }
}

async fn reserve_policy(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(snapshot) = idempotency::reserve(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        UPDATE_POLICY_OPERATION,
        request,
    )
    .await?
    else {
        return Ok(None);
    };
    snapshot
        .pointer("/data/id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(Some)
        .ok_or_else(invalid_policy_snapshot)
}

async fn complete_policy(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    request: &IdempotencyRequest,
    policy_id: Uuid,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        UPDATE_POLICY_OPERATION,
        request,
        200,
        json!({ "data": { "id": policy_id } }),
    )
    .await
}

fn invalid_policy_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "completed Analytics Policy idempotency record is invalid"
    ))
}

fn invalid_identity_link_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "completed Analytics Identity Link record is invalid"
    ))
}

fn invalid_erasure_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "completed Analytics Erasure Request record is invalid"
    ))
}

fn fact_payload_uuid(payload: &Value, field: &'static str) -> Result<Uuid, ApplicationError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
        .ok_or_else(corrupt_commerce_fact_event)
}

fn corrupt_commerce_fact_event() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "Analytics commerce fact event does not match authoritative commerce state"
    ))
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

fn properties(value: &BrowserEventProperties) -> Value {
    match value {
        BrowserEventProperties::PageViewed {
            path,
            title,
            referrer_domain,
            campaign_source,
            campaign_medium,
            campaign_name,
        } => json!({
            "path": path,
            "title": title,
            "referrer_domain": referrer_domain,
            "campaign_source": campaign_source,
            "campaign_medium": campaign_medium,
            "campaign_name": campaign_name,
        }),
        BrowserEventProperties::ProductViewed {
            product_id,
            product_variant_id,
        } => json!({
            "product_id": product_id.as_uuid(),
            "product_variant_id": product_variant_id.map(|id| id.as_uuid()),
        }),
        BrowserEventProperties::SearchPerformed {
            query,
            result_count,
        } => json!({
            "query": query,
            "result_count": result_count,
        }),
        BrowserEventProperties::CartLineAdded {
            cart_id,
            product_variant_id,
            quantity,
        } => json!({
            "cart_id": cart_id.as_uuid(),
            "product_variant_id": product_variant_id.as_uuid(),
            "quantity": quantity,
        }),
        BrowserEventProperties::CheckoutStarted {
            cart_id,
            checkout_id,
        } => json!({
            "cart_id": cart_id.as_uuid(),
            "checkout_id": checkout_id.map(|id| id.as_uuid()),
        }),
        BrowserEventProperties::EngagementHeartbeat {
            page_view_event_id,
            active_milliseconds,
        } => json!({
            "page_view_event_id": page_view_event_id,
            "active_milliseconds": active_milliseconds,
        }),
    }
}

struct AttributionColumns<'a> {
    landing_path: Option<&'a str>,
    referrer_domain: Option<&'a str>,
    campaign_source: Option<&'a str>,
    campaign_medium: Option<&'a str>,
    campaign_name: Option<&'a str>,
    cart_id: Option<Uuid>,
    checkout_id: Option<Uuid>,
}

fn attribution_columns(properties: &BrowserEventProperties) -> AttributionColumns<'_> {
    if let BrowserEventProperties::PageViewed {
        path,
        referrer_domain,
        campaign_source,
        campaign_medium,
        campaign_name,
        ..
    } = properties
    {
        AttributionColumns {
            landing_path: Some(path),
            referrer_domain: referrer_domain.as_deref(),
            campaign_source: campaign_source.as_deref(),
            campaign_medium: campaign_medium.as_deref(),
            campaign_name: campaign_name.as_deref(),
            cart_id: None,
            checkout_id: None,
        }
    } else if let BrowserEventProperties::CartLineAdded { cart_id, .. } = properties {
        AttributionColumns {
            landing_path: None,
            referrer_domain: None,
            campaign_source: None,
            campaign_medium: None,
            campaign_name: None,
            cart_id: Some(cart_id.as_uuid()),
            checkout_id: None,
        }
    } else if let BrowserEventProperties::CheckoutStarted {
        cart_id,
        checkout_id,
    } = properties
    {
        AttributionColumns {
            landing_path: None,
            referrer_domain: None,
            campaign_source: None,
            campaign_medium: None,
            campaign_name: None,
            cart_id: Some(cart_id.as_uuid()),
            checkout_id: checkout_id.map(|id| id.as_uuid()),
        }
    } else {
        AttributionColumns {
            landing_path: None,
            referrer_domain: None,
            campaign_source: None,
            campaign_medium: None,
            campaign_name: None,
            cart_id: None,
            checkout_id: None,
        }
    }
}

async fn refresh_behavior_report(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    sales_channel_id: SalesChannelId,
    report_date: time::Date,
    refreshed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "DELETE FROM analytics.daily_behavior_reports \
         WHERE merchant_account_id = $1 AND store_id = $2 \
           AND sales_channel_id = $3 AND report_date = $4",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(sales_channel_id.as_uuid())
    .bind(report_date)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO analytics.daily_behavior_reports \
         (merchant_account_id, store_id, sales_channel_id, report_date, sessions, events, \
          page_views, product_views, searches, cart_line_additions, checkouts_started, \
          active_engagement_milliseconds, refreshed_at) \
         SELECT $1,$2,$3,$4,count(*),sum(event_count),sum(page_view_count), \
                sum(product_view_count),sum(search_count),sum(cart_line_added_count), \
                sum(checkout_started_count),sum(active_engagement_milliseconds),$5 \
           FROM analytics.sessions \
          WHERE merchant_account_id = $1 AND store_id = $2 AND sales_channel_id = $3 \
            AND (started_at AT TIME ZONE 'UTC')::date = $4 \
         HAVING count(*) > 0",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(sales_channel_id.as_uuid())
    .bind(report_date)
    .bind(refreshed_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn refresh_commerce_report(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    report_date: time::Date,
    refreshed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "DELETE FROM analytics.daily_commerce_reports \
         WHERE merchant_account_id = $1 AND store_id = $2 AND report_date = $3",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(report_date)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO analytics.daily_commerce_reports \
         (merchant_account_id, store_id, sales_channel_id, report_date, currency, \
          orders_created, order_amount_minor, payments_captured, captured_amount_minor, \
          refunds_succeeded, refunded_amount_minor, fulfillments_shipped, returns_completed, \
          refreshed_at) \
         SELECT fact.merchant_account_id, fact.store_id, fact.sales_channel_id, $3, \
                orders.currency, \
                count(*) FILTER (WHERE fact.fact_name = 'order_created'), \
                coalesce(sum(fact.amount_minor) FILTER (WHERE fact.fact_name = 'order_created'),0), \
                count(*) FILTER (WHERE fact.fact_name = 'payment_captured'), \
                coalesce(sum(fact.amount_minor) FILTER (WHERE fact.fact_name = 'payment_captured'),0), \
                count(*) FILTER (WHERE fact.fact_name = 'refund_succeeded'), \
                coalesce(sum(fact.amount_minor) FILTER (WHERE fact.fact_name = 'refund_succeeded'),0), \
                count(*) FILTER (WHERE fact.fact_name = 'fulfillment_shipped'), \
                count(*) FILTER (WHERE fact.fact_name = 'return_completed'), $4 \
           FROM analytics.commerce_facts AS fact \
           INNER JOIN sales.orders AS orders \
             ON orders.merchant_account_id = fact.merchant_account_id \
            AND orders.store_id = fact.store_id AND orders.id = fact.order_id \
          WHERE fact.merchant_account_id = $1 AND fact.store_id = $2 \
            AND (fact.occurred_at AT TIME ZONE 'UTC')::date = $3 \
          GROUP BY fact.merchant_account_id, fact.store_id, fact.sales_channel_id, orders.currency",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(report_date)
    .bind(refreshed_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn refresh_attribution_report(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    commerce_fact_id: Uuid,
    refreshed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let report_date: time::Date = sqlx::query_scalar(
        "SELECT (occurred_at AT TIME ZONE 'UTC')::date FROM analytics.commerce_facts \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(commerce_fact_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "DELETE FROM analytics.daily_attribution_reports \
         WHERE merchant_account_id = $1 AND store_id = $2 AND report_date = $3",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(report_date)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO analytics.daily_attribution_reports \
         (merchant_account_id, store_id, sales_channel_id, report_date, attribution_model, \
          model_version, is_direct, campaign_source, campaign_medium, campaign_name, \
          attributed_orders, attributed_amount_minor, currency, refreshed_at) \
         SELECT result.merchant_account_id, result.store_id, result.sales_channel_id, $3, \
                result.attribution_model, result.model_version, result.is_direct, \
                coalesce(result.campaign_source,''), coalesce(result.campaign_medium,''), \
                coalesce(result.campaign_name,''), count(*), sum(fact.amount_minor), \
                fact.currency, $4 \
           FROM analytics.attribution_results AS result \
           INNER JOIN analytics.commerce_facts AS fact \
             ON fact.merchant_account_id = result.merchant_account_id \
            AND fact.store_id = result.store_id AND fact.id = result.commerce_fact_id \
          WHERE result.merchant_account_id = $1 AND result.store_id = $2 \
            AND (fact.occurred_at AT TIME ZONE 'UTC')::date = $3 \
          GROUP BY result.merchant_account_id, result.store_id, result.sales_channel_id, \
                   result.attribution_model, result.model_version, result.is_direct, \
                   coalesce(result.campaign_source,''), coalesce(result.campaign_medium,''), \
                   coalesce(result.campaign_name,''), fact.currency",
    )
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(report_date)
    .bind(refreshed_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn behavior_report(row: DailyBehaviorReportRow) -> Result<DailyBehaviorReport, ApplicationError> {
    Ok(DailyBehaviorReport {
        sales_channel_id: SalesChannelId::from_uuid(row.sales_channel_id),
        report_date: row.report_date,
        sessions: u64::try_from(row.sessions).map_err(conversion_error)?,
        events: u64::try_from(row.events).map_err(conversion_error)?,
        page_views: u64::try_from(row.page_views).map_err(conversion_error)?,
        product_views: u64::try_from(row.product_views).map_err(conversion_error)?,
        searches: u64::try_from(row.searches).map_err(conversion_error)?,
        cart_line_additions: u64::try_from(row.cart_line_additions).map_err(conversion_error)?,
        checkouts_started: u64::try_from(row.checkouts_started).map_err(conversion_error)?,
        active_engagement_milliseconds: u64::try_from(row.active_engagement_milliseconds)
            .map_err(conversion_error)?,
        refreshed_at: row.refreshed_at,
    })
}

fn commerce_report(row: DailyCommerceReportRow) -> Result<DailyCommerceReport, ApplicationError> {
    Ok(DailyCommerceReport {
        sales_channel_id: SalesChannelId::from_uuid(row.sales_channel_id),
        report_date: row.report_date,
        currency: row.currency,
        orders_created: u64::try_from(row.orders_created).map_err(conversion_error)?,
        order_amount_minor: u64::try_from(row.order_amount_minor).map_err(conversion_error)?,
        payments_captured: u64::try_from(row.payments_captured).map_err(conversion_error)?,
        captured_amount_minor: u64::try_from(row.captured_amount_minor)
            .map_err(conversion_error)?,
        refunds_succeeded: u64::try_from(row.refunds_succeeded).map_err(conversion_error)?,
        refunded_amount_minor: u64::try_from(row.refunded_amount_minor)
            .map_err(conversion_error)?,
        fulfillments_shipped: u64::try_from(row.fulfillments_shipped).map_err(conversion_error)?,
        returns_completed: u64::try_from(row.returns_completed).map_err(conversion_error)?,
        refreshed_at: row.refreshed_at,
    })
}

fn attribution_report(
    row: DailyAttributionReportRow,
) -> Result<DailyAttributionReport, ApplicationError> {
    Ok(DailyAttributionReport {
        sales_channel_id: SalesChannelId::from_uuid(row.sales_channel_id),
        report_date: row.report_date,
        attribution_model: row.attribution_model,
        model_version: u16::try_from(row.model_version).map_err(conversion_error)?,
        is_direct: row.is_direct,
        campaign_source: nonempty(row.campaign_source),
        campaign_medium: nonempty(row.campaign_medium),
        campaign_name: nonempty(row.campaign_name),
        attributed_orders: u64::try_from(row.attributed_orders).map_err(conversion_error)?,
        attributed_amount_minor: u64::try_from(row.attributed_amount_minor)
            .map_err(conversion_error)?,
        currency: row.currency,
        refreshed_at: row.refreshed_at,
    })
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
