use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        AnalyticsEventRepository, AnalyticsPolicyRepository, AnalyticsRetentionPurgeResult,
        AnalyticsSessionizationJob, AnalyticsSessionizationQueue, IdempotencyRequest, MachineActor,
        ResolvedAnalyticsPolicy, StoreAnalyticsPolicy,
    },
};
use chaos_domain::{
    analytics::{
        AnalyticsPolicy, BrowserEvent, BrowserEventName, BrowserEventProperties,
        SESSION_INACTIVITY_MINUTES, SessionEventContribution, capped_session_engagement,
    },
    identity::UserId,
    merchant::{SalesChannelId, StoreId},
};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const UPDATE_POLICY_OPERATION: &str = "analytics_policies.update.v1";

#[derive(Clone)]
pub struct PostgresAnalyticsEventRepository {
    pool: PgPool,
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
            let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
                "INSERT INTO analytics.behavior_events \
                 (id, event_id, merchant_account_id, store_id, sales_channel_id, event_name, \
                  schema_version, source, anonymous_id, session_id, analytics_storage_consent, \
                  advertising_storage_consent, consent_policy_version, \
                  collection_policy_version, properties, occurred_at, received_at, \
                  retention_expires_at) \
                 VALUES (uuidv7(),$1,$2,$3,$4,$5::analytics.browser_event_name,$6, \
                         'browser',$7,$8,true,$9,$10,$11,$12,$13,$14,$15) \
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
            .bind(event.consent().policy_version())
            .bind(collection_policy_version)
            .bind(properties(event.properties()))
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
        let (behavior_events_deleted, sessions_deleted): (i64, i64) =
            sqlx::query_as("SELECT * FROM analytics.purge_expired_data($1,$2)")
                .bind(i32::from(limit))
                .bind(now)
                .fetch_one(&self.pool)
                .await
                .map_err(database_error)?;
        Ok(AnalyticsRetentionPurgeResult {
            behavior_events_deleted: u64::try_from(behavior_events_deleted)
                .map_err(conversion_error)?,
            sessions_deleted: u64::try_from(sessions_deleted).map_err(conversion_error)?,
        })
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
        } => json!({
            "path": path,
            "title": title,
            "referrer_domain": referrer_domain,
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

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
