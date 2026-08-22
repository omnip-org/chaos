use async_trait::async_trait;
use chaos_application::{ApplicationError, ports::*, store::StoreActor};
use chaos_domain::{
    analytics::{
        AnalyticsSettings, BrowserCollectionMode, BrowserEvent, BrowserEventProperties,
        TrafficAttribution, TrafficTouchpoint,
    },
    identity::UserId,
    store::StoreId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const UPDATE_SETTINGS_OPERATION: &str = "analytics.update_settings";
const CONFIGURE_DESTINATION_OPERATION: &str = "analytics.configure_destination";

pub struct PostgresAnalyticsEventRepository {
    pool: PgPool,
}
impl PostgresAnalyticsEventRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(FromRow)]
struct SettingsRow {
    store_id: Uuid,
    revision: i32,
    collection_enabled: bool,
    browser_collection_mode: String,
    provider_reporting_enabled: bool,
    updated_by: Uuid,
    updated_at: OffsetDateTime,
}
#[derive(Deserialize, Serialize)]
struct SettingsSnapshot {
    store_id: Uuid,
    revision: i32,
    collection_enabled: bool,
    browser_collection_mode: String,
    provider_reporting_enabled: bool,
    updated_by: Option<Uuid>,
    updated_at: Option<OffsetDateTime>,
}

#[derive(Deserialize, Serialize)]
struct DestinationSnapshot {
    id: Uuid,
    store_id: Uuid,
    provider: String,
    external_account_reference: String,
    enabled: bool,
    credentials_configured: bool,
    configuration: Value,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct DeliverySnapshot {
    provider: String,
    status: String,
    delivered_at: Option<OffsetDateTime>,
    provider_reference: Option<String>,
    last_error: Option<String>,
}

async fn context(
    tx: &mut Transaction<'_, Postgres>,
    store: Uuid,
    user: Option<Uuid>,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT set_config('app.store_id',$1,true),set_config('app.user_id',$2,true)")
        .bind(store.to_string())
        .bind(user.map_or_else(String::new, |id| id.to_string()))
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}
fn row_settings_value(r: &SettingsRow) -> Result<AnalyticsSettings, ApplicationError> {
    AnalyticsSettings::new(
        r.collection_enabled,
        match r.browser_collection_mode.as_str() {
            "opt_in" => BrowserCollectionMode::OptIn,
            "opt_out" => BrowserCollectionMode::OptOut,
            _ => {
                return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                    "invalid browser collection mode"
                )));
            }
        },
        r.provider_reporting_enabled,
    )
    .map_err(Into::into)
}
fn row_settings(r: SettingsRow) -> Result<StoreAnalyticsSettings, ApplicationError> {
    Ok(StoreAnalyticsSettings {
        store_id: StoreId::from_uuid(r.store_id),
        revision: r.revision,
        settings: row_settings_value(&r)?,
        updated_by: Some(UserId::from_uuid(r.updated_by)),
        updated_at: Some(r.updated_at),
    })
}
async fn load_settings(
    tx: &mut Transaction<'_, Postgres>,
    store: StoreId,
) -> Result<Option<SettingsRow>, ApplicationError> {
    sqlx::query_as("SELECT store_id,revision,collection_enabled,browser_collection_mode::text,provider_reporting_enabled,updated_by,updated_at FROM integration.analytics_policy WHERE store_id=$1").bind(store.as_uuid()).fetch_optional(&mut **tx).await.map_err(db)
}

#[async_trait]
impl AnalyticsEventRepository for PostgresAnalyticsEventRepository {
    async fn resolve_collection_settings(
        &self,
        actor: &MachineActor,
        _: OffsetDateTime,
    ) -> Result<ResolvedAnalyticsSettings, ApplicationError> {
        let channel = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, actor.store_id.as_uuid(), None).await?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commerce.stores s JOIN commerce.sales_channels c ON c.store_id=s.id WHERE s.id=$1 AND c.id=$2 AND s.status='active' AND c.status='active')").bind(actor.store_id.as_uuid()).bind(channel.as_uuid()).fetch_one(&mut *tx).await.map_err(db)?;
        if !active {
            return Err(ApplicationError::Forbidden);
        }
        let result = match load_settings(&mut tx, actor.store_id).await? {
            Some(row) => ResolvedAnalyticsSettings {
                revision: row.revision,
                settings: row_settings_value(&row)?,
            },
            None => ResolvedAnalyticsSettings {
                revision: 1,
                settings: AnalyticsSettings::builtin(),
            },
        };
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
    async fn append_browser_events(
        &self,
        actor: &MachineActor,
        events: &[BrowserEvent],
        revision: i32,
        browser_collection_mode: BrowserCollectionMode,
        provider_reporting_enabled: bool,
        received: OffsetDateTime,
    ) -> Result<usize, ApplicationError> {
        let channel = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, actor.store_id.as_uuid(), None).await?;
        let provider_enabled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM integration.analytics_destinations WHERE store_id=$1 AND enabled)").bind(actor.store_id.as_uuid()).fetch_one(&mut *tx).await.map_err(db)?;
        let mut count = 0;
        for event in events {
            let mut columns = browser_columns(event.properties());
            if let Some(traffic) = event.traffic() {
                columns.properties["traffic"] = traffic_json(traffic);
            }
            let eligible = provider_reporting_enabled
                && provider_enabled
                && (event.consent().advertising_storage()
                    || (event.collection_basis()
                        == chaos_domain::analytics::BrowserCollectionBasis::StorePolicy
                        && browser_collection_mode == BrowserCollectionMode::OptOut));
            // Serialize the global event-id check across daily partitions.
            sqlx::query(
                "SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':' || $2::text, 0))",
            )
            .bind(actor.store_id.as_uuid())
            .bind(event.event_id())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            let duplicate: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM integration.analytics_events WHERE store_id=$1 AND event_id=$2)",
            )
            .bind(actor.store_id.as_uuid())
            .bind(event.event_id())
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
            let id: Option<Uuid> = if duplicate {
                None
            } else {
                sqlx::query_scalar("INSERT INTO integration.analytics_events (id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,shopper_id,session_id,product_id,product_variant_id,cart_id,checkout_id,path,analytics_storage_consent,advertising_storage_consent,provider_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at) VALUES(uuidv7(),$1,$2,$3,$4::integration.analytics_event_name,'browser',$5::integration.browser_collection_basis,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21) RETURNING id")
                    .bind(event.event_id())
                    .bind(actor.store_id.as_uuid())
                    .bind(channel.as_uuid())
                    .bind(event.name().as_str())
                    .bind(event.collection_basis().as_str())
                    .bind(i16::try_from(event.schema_version()).map_err(convert)?)
                    .bind(event.shopper_id())
                    .bind(event.session_id())
                    .bind(columns.product_id)
                    .bind(columns.product_variant_id)
                    .bind(columns.cart_id)
                    .bind(columns.checkout_id)
                    .bind(columns.path)
                    .bind(event.consent().analytics_storage())
                    .bind(event.consent().advertising_storage())
                    .bind(eligible)
                    .bind(event.consent().policy_version())
                    .bind(revision)
                    .bind(columns.properties)
                    .bind(event.occurred_at())
                    .bind(received)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db)?
            };
            if id.is_some() {
                count += 1;
            }
        }
        tx.commit().await.map_err(db)?;
        Ok(count)
    }
}

#[async_trait]
impl AnalyticsSettingsRepository for PostgresAnalyticsEventRepository {
    async fn get_settings(
        &self,
        actor: StoreActor,
        store: StoreId,
    ) -> Result<Option<StoreAnalyticsSettings>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commerce.stores WHERE id=$1)")
                .bind(store.as_uuid())
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        if !exists {
            return Ok(None);
        }
        let value = load_settings(&mut tx, store)
            .await?
            .map(row_settings)
            .transpose()?
            .unwrap_or(StoreAnalyticsSettings {
                store_id: store,
                revision: 1,
                settings: AnalyticsSettings::builtin(),
                updated_by: None,
                updated_at: None,
            });
        tx.commit().await.map_err(db)?;
        Ok(Some(value))
    }
    async fn update_settings(
        &self,
        actor: StoreActor,
        store: StoreId,
        p: AnalyticsSettings,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreAnalyticsSettings, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            UPDATE_SETTINGS_OPERATION,
            request,
        )
        .await?
        {
            let result = settings_from_snapshot(snapshot)?;
            tx.commit().await.map_err(db)?;
            return Ok(result);
        }
        let r:SettingsRow=sqlx::query_as("INSERT INTO integration.analytics_policy(store_id,revision,collection_enabled,browser_collection_mode,provider_reporting_enabled,updated_by,updated_at) VALUES($1,1,$2,$3::integration.browser_collection_mode,$4,$5,$6) ON CONFLICT(store_id) DO UPDATE SET revision=integration.analytics_policy.revision+1,collection_enabled=EXCLUDED.collection_enabled,browser_collection_mode=EXCLUDED.browser_collection_mode,provider_reporting_enabled=EXCLUDED.provider_reporting_enabled,updated_by=EXCLUDED.updated_by,updated_at=EXCLUDED.updated_at RETURNING store_id,revision,collection_enabled,browser_collection_mode::text,provider_reporting_enabled,updated_by,updated_at").bind(store.as_uuid()).bind(p.collection_enabled()).bind(p.browser_collection_mode().as_str()).bind(p.provider_reporting_enabled()).bind(actor.user_id().as_uuid()).bind(now).fetch_one(&mut *tx).await.map_err(db)?;
        let result = row_settings(r)?;
        idempotency::complete(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            UPDATE_SETTINGS_OPERATION,
            request,
            200,
            settings_snapshot(&result)?,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
}

#[async_trait]
impl AnalyticsEventQueryRepository for PostgresAnalyticsEventRepository {
    async fn list_events(
        &self,
        actor: StoreActor,
        store: StoreId,
        query: AnalyticsEventQuery,
        limit: u16,
    ) -> Result<AnalyticsEventPage, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let query_limit = i32::from(limit) + 1;
        let rows: Vec<(
            Uuid,
            Uuid,
            String,
            String,
            Uuid,
            OffsetDateTime,
            OffsetDateTime,
            bool,
            sqlx::types::Json<Vec<DeliverySnapshot>>,
        )> = sqlx::query_as(
            "SELECT e.id,e.event_id,e.event_name::text,e.source::text,e.shopper_id,e.occurred_at,e.received_at,
                    e.provider_eligible,
                    COALESCE(jsonb_agg(jsonb_build_object(
                        'provider', c.provider,
                        'status', d.delivery_status::text,
                        'delivered_at', d.delivered_at,
                        'provider_reference', d.provider_reference,
                        'last_error', d.last_error
                    ) ORDER BY c.provider) FILTER (WHERE d.id IS NOT NULL), '[]'::jsonb)
             FROM integration.analytics_events e
             LEFT JOIN integration.analytics_deliveries d
               ON d.store_id=e.store_id AND d.analytics_event_id=e.id
             LEFT JOIN integration.analytics_destinations c
               ON c.store_id=d.store_id AND c.id=d.destination_id
             WHERE e.store_id=$1
               AND ($3::uuid IS NULL OR e.id < $3)
               AND ($4::text IS NULL OR e.event_name::text=$4)
               AND ($5::text IS NULL OR e.source::text=$5)
               AND ($6::text IS NULL OR EXISTS (
                   SELECT 1 FROM integration.analytics_deliveries filter_delivery
                    WHERE filter_delivery.store_id=e.store_id
                      AND filter_delivery.analytics_event_id=e.id
                      AND filter_delivery.delivery_status::text=$6
               ))
               AND ($7::uuid IS NULL OR e.shopper_id=$7)
             GROUP BY e.id,e.event_id,e.event_name,e.source,e.shopper_id,e.occurred_at,e.received_at,e.provider_eligible
             ORDER BY e.id DESC
             LIMIT $2",
        )
        .bind(store.as_uuid())
        .bind(query_limit)
        .bind(query.before_id)
        .bind(query.event_name)
        .bind(query.source)
        .bind(query.delivery_status)
        .bind(query.shopper_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;

        let has_more = rows.len() > usize::from(limit);
        let events = rows
            .into_iter()
            .take(usize::from(limit))
            .map(
                |(
                    id,
                    event_id,
                    event_name,
                    source,
                    shopper_id,
                    occurred_at,
                    received_at,
                    provider_eligible,
                    deliveries,
                )| AnalyticsEventRecord {
                    id,
                    event_id,
                    event_name,
                    source,
                    shopper_id,
                    occurred_at,
                    received_at,
                    provider_eligible,
                    deliveries: deliveries
                        .0
                        .into_iter()
                        .map(|delivery| AnalyticsEventDelivery {
                            provider: delivery.provider,
                            status: delivery.status,
                            delivered_at: delivery.delivered_at,
                            provider_reference: delivery.provider_reference,
                            last_error: delivery.last_error,
                        })
                        .collect(),
                },
            )
            .collect();
        Ok(AnalyticsEventPage { events, has_more })
    }
}

#[async_trait]
impl AnalyticsDestinationRepository for PostgresAnalyticsEventRepository {
    async fn get_destination(
        &self,
        actor: StoreActor,
        store: StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsDestination>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let r:Option<(Uuid,String,String,String,Value,bool,OffsetDateTime,OffsetDateTime)>=sqlx::query_as("SELECT id,provider,external_account_reference,credential_secret_reference,configuration,enabled,created_at,updated_at FROM integration.analytics_destinations WHERE store_id=$1 AND provider=$2").bind(store.as_uuid()).bind(provider).fetch_optional(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(r.map(|r| AnalyticsDestination {
            id: r.0,
            store_id: store,
            provider: r.1,
            external_account_reference: r.2,
            enabled: r.5,
            credentials_configured: true,
            configuration: r.4,
            created_at: r.6,
            updated_at: r.7,
        }))
    }
    async fn configure_destination(
        &self,
        actor: StoreActor,
        store: StoreId,
        c: AnalyticsDestinationConfiguration,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsDestination, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            CONFIGURE_DESTINATION_OPERATION,
            request,
        )
        .await?
        {
            let result = destination_from_snapshot(snapshot)?;
            tx.commit().await.map_err(db)?;
            return Ok(result);
        }
        let r:(Uuid,String,String,String,Value,bool,OffsetDateTime,OffsetDateTime)=sqlx::query_as("INSERT INTO integration.analytics_destinations(id,store_id,provider,external_account_reference,credential_secret_reference,configuration,enabled,created_by,created_at,updated_at) VALUES(uuidv7(),$1,$2,$3,$4,$5,$6,$7,$8,$8) ON CONFLICT(store_id,provider) DO UPDATE SET external_account_reference=EXCLUDED.external_account_reference,credential_secret_reference=EXCLUDED.credential_secret_reference,configuration=EXCLUDED.configuration,enabled=EXCLUDED.enabled,updated_at=EXCLUDED.updated_at RETURNING id,provider,external_account_reference,credential_secret_reference,configuration,enabled,created_at,updated_at").bind(store.as_uuid()).bind(c.provider).bind(c.external_account_reference).bind(c.credential_secret_reference).bind(c.configuration).bind(c.enabled).bind(actor.user_id().as_uuid()).bind(now).fetch_one(&mut *tx).await.map_err(db)?;
        let result = AnalyticsDestination {
            id: r.0,
            store_id: store,
            provider: r.1,
            external_account_reference: r.2,
            credentials_configured: true,
            configuration: r.4,
            enabled: r.5,
            created_at: r.6,
            updated_at: r.7,
        };
        idempotency::complete(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            CONFIGURE_DESTINATION_OPERATION,
            request,
            200,
            destination_snapshot(&result)?,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
}

#[async_trait]
impl AnalyticsEventRecorderRepository for PostgresAnalyticsEventRepository {
    async fn claim_server_events(
        &self,
        limit: u16,
    ) -> Result<Vec<ServerCommerceEventJob>, ApplicationError> {
        let rows:Vec<(Uuid,Uuid,String,Uuid,Value,OffsetDateTime,i32)>=sqlx::query_as("SELECT id,store_id,event_type,aggregate_id,payload,occurred_at,attempts FROM integration.claim_analytics_events($1)").bind(i32::from(limit)).fetch_all(&self.pool).await.map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| ServerCommerceEventJob {
                id: r.0,
                store_id: StoreId::from_uuid(r.1),
                event_type: r.2,
                aggregate_id: r.3,
                payload: r.4,
                occurred_at: r.5,
                attempts: u32::try_from(r.6).unwrap_or(u32::MAX),
            })
            .collect())
    }
    async fn ingest_server_event(
        &self,
        job: &ServerCommerceEventJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, job.store_id.as_uuid(), None).await?;
        let provider_reporting_enabled:bool=sqlx::query_scalar("SELECT COALESCE((SELECT provider_reporting_enabled FROM integration.analytics_policy WHERE store_id=$1),false) AND EXISTS (SELECT 1 FROM integration.analytics_destinations WHERE store_id=$1 AND enabled)").bind(job.store_id.as_uuid()).fetch_one(&mut *tx).await.map_err(db)?;
        let _ = insert_server(&mut tx, job, now, provider_reporting_enabled).await?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }
    async fn finish_server_event(
        &self,
        job: &ServerCommerceEventJob,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (ok, e) = result.map_or_else(|e| (false, e), |_| (true, String::new()));
        let done: Option<bool> =
            sqlx::query_scalar("SELECT integration.finish_event_outbox($1,$2,$3,$4,8,$5)")
                .bind(job.id)
                .bind(i32::try_from(job.attempts).unwrap_or(i32::MAX))
                .bind(ok)
                .bind(e)
                .bind(now)
                .fetch_one(&self.pool)
                .await
                .map_err(db)?;
        if done == Some(true) {
            Ok(())
        } else {
            Err(ApplicationError::Conflict {
                code: "analytics_event_lease_lost",
                message: "the Analytics event lease is no longer owned by this worker",
            })
        }
    }
}

#[async_trait]
impl AnalyticsDeliveryRepository for PostgresAnalyticsEventRepository {
    async fn schedule_deliveries(&self, limit: u16) -> Result<usize, ApplicationError> {
        let scheduled: Option<i64> =
            sqlx::query_scalar("SELECT integration.schedule_analytics_deliveries($1)")
                .bind(i64::from(limit))
                .fetch_one(&self.pool)
                .await
                .map_err(db)?;
        Ok(usize::try_from(scheduled.unwrap_or_default()).unwrap_or(usize::MAX))
    }

    async fn claim_deliveries(
        &self,
        limit: u16,
    ) -> Result<Vec<AnalyticsDeliveryJob>, ApplicationError> {
        let r:Vec<(Uuid,Uuid,Uuid,Uuid,i32)>=sqlx::query_as("SELECT id,store_id,destination_id,analytics_event_id,attempts FROM integration.claim_analytics_deliveries($1)").bind(i32::from(limit)).fetch_all(&self.pool).await.map_err(db)?;
        Ok(r.into_iter()
            .map(|r| AnalyticsDeliveryJob {
                id: r.0,
                store_id: StoreId::from_uuid(r.1),
                destination_id: r.2,
                analytics_event_id: r.3,
                attempts: u32::try_from(r.4).unwrap_or(u32::MAX),
            })
            .collect())
    }
    async fn load_delivery(
        &self,
        job: &AnalyticsDeliveryJob,
    ) -> Result<AnalyticsDeliveryCommand, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, job.store_id.as_uuid(), None).await?;
        let r: (
            Uuid,
            String,
            String,
            String,
            Value,
            String,
            OffsetDateTime,
            Uuid,
            Option<String>,
            Option<i64>,
            Option<String>,
            Value,
        ) = sqlx::query_as(
            "SELECT e.event_id,destination.provider,destination.external_account_reference,destination.credential_secret_reference,destination.configuration,e.event_name::text,e.occurred_at,e.shopper_id,e.properties->>'source_url',e.value_minor,e.currency,e.properties || jsonb_strip_nulls(jsonb_build_object('content_ids',CASE WHEN e.product_variant_id IS NOT NULL THEN jsonb_build_array(e.product_variant_id::text) WHEN e.product_id IS NOT NULL THEN jsonb_build_array(e.product_id::text) END,'path',e.path,'order_id',e.order_id,'payment_attempt_id',e.payment_attempt_id,'refund_id',e.refund_id)) FROM integration.analytics_deliveries delivery JOIN integration.analytics_events e ON e.store_id=delivery.store_id AND e.id=delivery.analytics_event_id JOIN integration.analytics_destinations destination ON destination.store_id=delivery.store_id AND destination.id=delivery.destination_id WHERE delivery.store_id=$1 AND delivery.id=$2 AND delivery.delivery_status='pending' AND destination.enabled",
        )
        .bind(job.store_id.as_uuid())
        .bind(job.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(AnalyticsDeliveryCommand {
            delivery_id: job.id,
            provider: r.1,
            event_id: r.0,
            external_account_reference: r.2,
            credential_secret_reference: r.3,
            configuration: r.4,
            event_name: r.5,
            occurred_at: r.6,
            shopper_id: r.7,
            source_url: r.8,
            value_minor: r.9,
            currency: r.10,
            properties: r.11,
        })
    }
    async fn finish_delivery(
        &self,
        job: &AnalyticsDeliveryJob,
        result: Result<AnalyticsDeliveryReceipt, AnalyticsDeliveryError>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (ok, reference, error, retry) = match result {
            Ok(r) => (true, r.provider_reference, None, false),
            Err(e) => (false, None, Some(e.message), e.retryable),
        };
        let finished: Option<bool> = sqlx::query_scalar(
            "SELECT integration.finish_analytics_event_delivery($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(job.id)
        .bind(i32::try_from(job.attempts).unwrap_or(i32::MAX))
        .bind(ok)
        .bind(retry)
        .bind(reference)
        .bind(error)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        if finished == Some(true) {
            Ok(())
        } else {
            Err(ApplicationError::Conflict {
                code: "analytics_delivery_not_pending",
                message: "the Analytics delivery is no longer pending",
            })
        }
    }
}

async fn insert_server(
    tx: &mut Transaction<'_, Postgres>,
    job: &ServerCommerceEventJob,
    now: OffsetDateTime,
    provider_reporting_enabled: bool,
) -> Result<Option<Uuid>, ApplicationError> {
    let name = match job.event_type.as_str() {
        "analytics.cart.line_added" => "add_to_cart",
        "analytics.checkout.initiated" => "initiate_checkout",
        "analytics.payment.initiated" => "add_payment_info",
        "analytics.payment.captured" => "purchase",
        "analytics.refund.succeeded" => "refund",
        _ => {
            return Err(ApplicationError::Conflict {
                code: "unsupported_analytics_event",
                message: "the outbox event is not supported by Analytics",
            });
        }
    };
    // The partitioned ledger cannot enforce a global unique (store_id,
    // event_id) key. Serialize the check across partitions so retries remain
    // idempotent without another registry table.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':' || $2::text, 0))")
        .bind(job.store_id.as_uuid())
        .bind(job.aggregate_id)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    if name == "purchase" || name == "add_payment_info" {
        let expected_status = if name == "purchase" {
            "captured"
        } else {
            "any"
        };
        let query = "INSERT INTO integration.analytics_events(id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,shopper_id,checkout_id,order_id,payment_attempt_id,value_minor,currency,analytics_storage_consent,advertising_storage_consent,provider_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at) SELECT uuidv7(),CASE WHEN $6='purchase' THEN o.id ELSE a.id END,a.store_id,o.sales_channel_id,$6::integration.analytics_event_name,'server','server',1,o.shopper_id,o.checkout_id,o.id,a.id,a.amount_minor,a.currency,true,false,$3 AND COALESCE(s.browser_collection_mode='opt_out',true),NULL,COALESCE(s.revision,1),'{}'::jsonb,$1,$2 FROM commerce.payment_attempts a JOIN commerce.orders o ON o.store_id=a.store_id AND o.id=a.order_id LEFT JOIN integration.analytics_policy s ON s.store_id=a.store_id WHERE a.store_id=$4 AND a.id=$5 AND ($7 = 'any' OR a.status::text=$7) AND NOT EXISTS (SELECT 1 FROM integration.analytics_events duplicate WHERE duplicate.store_id=a.store_id AND duplicate.event_id=CASE WHEN $6='purchase' THEN o.id ELSE a.id END) RETURNING id";
        return sqlx::query_scalar(query)
            .bind(job.occurred_at)
            .bind(now)
            .bind(provider_reporting_enabled)
            .bind(job.store_id.as_uuid())
            .bind(job.aggregate_id)
            .bind(name)
            .bind(expected_status)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db);
    }
    if name == "refund" {
        let query = "INSERT INTO integration.analytics_events(id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,shopper_id,checkout_id,order_id,payment_attempt_id,refund_id,value_minor,currency,analytics_storage_consent,advertising_storage_consent,provider_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at) SELECT uuidv7(),$1,r.store_id,o.sales_channel_id,'refund','server','server',1,o.shopper_id,o.checkout_id,o.id,a.id,r.id,r.amount_minor,r.currency,true,false,$4 AND COALESCE(s.browser_collection_mode='opt_out',true),NULL,COALESCE(s.revision,1),'{}'::jsonb,$2,$3 FROM commerce.refunds r JOIN commerce.payment_attempts a ON a.store_id=r.store_id AND a.id=r.payment_attempt_id JOIN commerce.orders o ON o.store_id=a.store_id AND o.id=a.order_id LEFT JOIN integration.analytics_policy s ON s.store_id=r.store_id WHERE r.store_id=$5 AND r.id=$6 AND r.status='succeeded' AND NOT EXISTS (SELECT 1 FROM integration.analytics_events duplicate WHERE duplicate.store_id=r.store_id AND duplicate.event_id=$1) RETURNING id";
        return sqlx::query_scalar(query)
            .bind(job.aggregate_id)
            .bind(job.occurred_at)
            .bind(now)
            .bind(provider_reporting_enabled)
            .bind(job.store_id.as_uuid())
            .bind(job.aggregate_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db);
    }
    if name == "add_to_cart" {
        let variant_id = job
            .payload
            .get("product_variant_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| ApplicationError::Conflict {
                code: "invalid_analytics_event_payload",
                message: "the AddToCart event is missing a Product Variant",
            })?;
        let query = "INSERT INTO integration.analytics_events(id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,shopper_id,cart_id,product_variant_id,analytics_storage_consent,advertising_storage_consent,provider_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at) SELECT uuidv7(),$1,c.store_id,c.sales_channel_id,'add_to_cart','server','server',1,c.shopper_id,c.id,$2,true,false,$3 AND COALESCE(s.browser_collection_mode='opt_out',true),NULL,COALESCE(s.revision,1),$4,$5,$6 FROM commerce.carts c LEFT JOIN integration.analytics_policy s ON s.store_id=c.store_id WHERE c.store_id=$7 AND c.id=$8 AND NOT EXISTS (SELECT 1 FROM integration.analytics_events duplicate WHERE duplicate.store_id=c.store_id AND duplicate.event_id=$1) RETURNING id";
        return sqlx::query_scalar(query)
            .bind(job.id)
            .bind(variant_id)
            .bind(provider_reporting_enabled)
            .bind(&job.payload)
            .bind(job.occurred_at)
            .bind(now)
            .bind(job.store_id.as_uuid())
            .bind(job.aggregate_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db);
    }
    let query = "INSERT INTO integration.analytics_events(id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,shopper_id,cart_id,checkout_id,value_minor,currency,analytics_storage_consent,advertising_storage_consent,provider_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at) SELECT uuidv7(),$1,c.store_id,c.sales_channel_id,'initiate_checkout','server','server',1,c.shopper_id,c.cart_id,c.id,c.total_amount_minor,c.currency,true,false,$2 AND COALESCE(s.browser_collection_mode='opt_out',true),NULL,COALESCE(s.revision,1),$3,$4,$5 FROM commerce.checkouts c LEFT JOIN integration.analytics_policy s ON s.store_id=c.store_id WHERE c.store_id=$6 AND c.id=$7 AND NOT EXISTS (SELECT 1 FROM integration.analytics_events duplicate WHERE duplicate.store_id=c.store_id AND duplicate.event_id=$1) RETURNING id";
    sqlx::query_scalar(query)
        .bind(job.id)
        .bind(provider_reporting_enabled)
        .bind(&job.payload)
        .bind(job.occurred_at)
        .bind(now)
        .bind(job.store_id.as_uuid())
        .bind(job.aggregate_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)
}
struct BrowserColumns {
    product_id: Option<Uuid>,
    product_variant_id: Option<Uuid>,
    cart_id: Option<Uuid>,
    checkout_id: Option<Uuid>,
    path: Option<String>,
    properties: Value,
}

fn browser_columns(p: &BrowserEventProperties) -> BrowserColumns {
    let (product_id, product_variant_id, cart_id, checkout_id, path, properties) = match p {
        BrowserEventProperties::PageView {
            path,
            title,
            referrer_domain,
        } => (
            None,
            None,
            None,
            None,
            Some(path.clone()),
            json!({"title":title,"referrer_domain":referrer_domain}),
        ),
        BrowserEventProperties::ViewContent {
            product_id,
            product_variant_id,
        } => (
            Some(product_id.as_uuid()),
            product_variant_id.map(|v| v.as_uuid()),
            None,
            None,
            None,
            json!({}),
        ),
        BrowserEventProperties::Search {
            query,
            result_count,
        } => (
            None,
            None,
            None,
            None,
            None,
            json!({"query":query,"result_count":result_count}),
        ),
        BrowserEventProperties::AddToCart {
            cart_id,
            product_variant_id,
            quantity,
        } => (
            None,
            Some(product_variant_id.as_uuid()),
            Some(cart_id.as_uuid()),
            None,
            None,
            json!({"quantity":quantity}),
        ),
        BrowserEventProperties::InitiateCheckout {
            cart_id,
            checkout_id,
        } => (
            None,
            None,
            Some(cart_id.as_uuid()),
            checkout_id.map(|v| v.as_uuid()),
            None,
            json!({}),
        ),
        BrowserEventProperties::ViewDuration {
            page_view_event_id,
            active_milliseconds,
        } => (
            None,
            None,
            None,
            None,
            None,
            json!({"page_view_event_id":page_view_event_id,"active_milliseconds":active_milliseconds}),
        ),
    };
    BrowserColumns {
        product_id,
        product_variant_id,
        cart_id,
        checkout_id,
        path,
        properties,
    }
}

fn traffic_json(value: &TrafficAttribution) -> Value {
    json!({
        "first": touchpoint_json(value.first()),
        "session": touchpoint_json(value.session()),
        "last_non_direct": value.last_non_direct().map(touchpoint_json),
    })
}

fn touchpoint_json(value: &TrafficTouchpoint) -> Value {
    let [
        source,
        medium,
        campaign,
        campaign_id,
        term,
        content,
        referrer_domain,
        fbclid,
        gclid,
    ] = value.fields();
    json!({
        "source": source,
        "medium": medium,
        "campaign": campaign,
        "campaign_id": campaign_id,
        "term": term,
        "content": content,
        "referrer_domain": referrer_domain,
        "fbclid": fbclid,
        "gclid": gclid,
    })
}
fn settings_snapshot(item: &StoreAnalyticsSettings) -> Result<Value, ApplicationError> {
    snapshot_value(SettingsSnapshot {
        store_id: item.store_id.as_uuid(),
        revision: item.revision,
        collection_enabled: item.settings.collection_enabled(),
        browser_collection_mode: item.settings.browser_collection_mode().as_str().into(),
        provider_reporting_enabled: item.settings.provider_reporting_enabled(),
        updated_by: item.updated_by.map(|id| id.as_uuid()),
        updated_at: item.updated_at,
    })
}

fn settings_from_snapshot(value: Value) -> Result<StoreAnalyticsSettings, ApplicationError> {
    let item: SettingsSnapshot = parse_snapshot(value)?;
    Ok(StoreAnalyticsSettings {
        store_id: StoreId::from_uuid(item.store_id),
        revision: item.revision,
        settings: AnalyticsSettings::new(
            item.collection_enabled,
            match item.browser_collection_mode.as_str() {
                "opt_in" => BrowserCollectionMode::OptIn,
                "opt_out" => BrowserCollectionMode::OptOut,
                _ => {
                    return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                        "invalid browser collection mode"
                    )));
                }
            },
            item.provider_reporting_enabled,
        )?,
        updated_by: item.updated_by.map(UserId::from_uuid),
        updated_at: item.updated_at,
    })
}

fn destination_snapshot(item: &AnalyticsDestination) -> Result<Value, ApplicationError> {
    snapshot_value(DestinationSnapshot {
        id: item.id,
        store_id: item.store_id.as_uuid(),
        provider: item.provider.clone(),
        external_account_reference: item.external_account_reference.clone(),
        enabled: item.enabled,
        credentials_configured: item.credentials_configured,
        configuration: item.configuration.clone(),
        created_at: item.created_at,
        updated_at: item.updated_at,
    })
}

fn destination_from_snapshot(value: Value) -> Result<AnalyticsDestination, ApplicationError> {
    let item: DestinationSnapshot = parse_snapshot(value)?;
    Ok(AnalyticsDestination {
        id: item.id,
        store_id: StoreId::from_uuid(item.store_id),
        provider: item.provider,
        external_account_reference: item.external_account_reference,
        enabled: item.enabled,
        credentials_configured: item.credentials_configured,
        configuration: item.configuration,
        created_at: item.created_at,
        updated_at: item.updated_at,
    })
}

fn snapshot_value(item: impl Serialize) -> Result<Value, ApplicationError> {
    serde_json::to_value(item).map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn parse_snapshot<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ApplicationError> {
    serde_json::from_value(value).map_err(|_| invalid_snapshot())
}

fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("invalid Analytics idempotency snapshot"))
}

fn db(e: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(e.into())
}
fn convert(e: impl std::error::Error + Send + Sync + 'static) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::Error::new(e))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chaos_application::store::StoreQueries;
    use chaos_domain::{
        analytics::{BrowserEvent, BrowserEventProperties, ConsentSnapshot},
        identity::{AccessKeyId, UserId},
        store::{PublishableKeyId, SalesChannelId, StoreId},
    };
    use sqlx::postgres::PgPoolOptions;
    use time::Duration;

    use crate::repositories::PostgresStoreReadRepository;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn ledger_deduplicates_events_and_meta_claims_are_consent_bound_and_exclusive() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await
            .unwrap();
        let repository = PostgresAnalyticsEventRepository::new(pool.clone());
        let store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let user_id = UserId::new();
        let now = OffsetDateTime::now_utc();
        let store_uuid = store_id.as_uuid().simple().to_string();
        let store_code = format!("analytics-{}", &store_uuid[24..]);

        sqlx::query("INSERT INTO identity.users(id,email) VALUES($1,$2)")
            .bind(user_id.as_uuid())
            .bind(format!(
                "analytics-{}@example.com",
                user_id.as_uuid().simple()
            ))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO commerce.stores(id,code,name,default_currency,status) VALUES($1,$2,$2,'USD','active')")
            .bind(store_id.as_uuid())
            .bind(store_code)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO commerce.store_memberships(store_id,user_id,role) VALUES($1,$2,'owner')",
        )
        .bind(store_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO commerce.sales_channels(id,store_id,code,name,kind,is_default) VALUES($1,$2,'web','Web','web',true)")
            .bind(channel_id.as_uuid()).bind(store_id.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO integration.analytics_destinations(id,store_id,provider,external_account_reference,credential_secret_reference,configuration,enabled,created_by,created_at,updated_at) VALUES(uuidv7(),$1,'meta','12345','env://CHAOS_ANALYTICS_SECRET_TEST','{}',true,$2,$3,$3)")
            .bind(store_id.as_uuid()).bind(user_id.as_uuid()).bind(now).execute(&pool).await.unwrap();

        let actor = MachineActor {
            publishable_key_id: PublishableKeyId::from_uuid(Uuid::now_v7()),
            store_id,
            sales_channel_id: Some(channel_id),
            created_by_user_id: user_id,
        };
        let consented = BrowserEvent::new(
            Uuid::now_v7(),
            1,
            now,
            Uuid::now_v7(),
            Uuid::now_v7(),
            ConsentSnapshot::new(true, true, "test-v1").unwrap(),
            chaos_domain::analytics::BrowserCollectionBasis::Consent,
            None,
            BrowserEventProperties::page_view("/products", None, None).unwrap(),
        )
        .unwrap();
        let stored = repository
            .append_browser_events(
                &actor,
                std::slice::from_ref(&consented),
                1,
                BrowserCollectionMode::OptIn,
                true,
                now,
            )
            .await
            .unwrap();
        assert_eq!(stored, 1);
        assert_eq!(
            repository
                .append_browser_events(
                    &actor,
                    &[consented],
                    1,
                    BrowserCollectionMode::OptIn,
                    true,
                    now,
                )
                .await
                .unwrap(),
            0
        );

        let unconsented = BrowserEvent::new(
            Uuid::now_v7(),
            1,
            now,
            Uuid::now_v7(),
            Uuid::now_v7(),
            ConsentSnapshot::new(true, false, "test-v1").unwrap(),
            chaos_domain::analytics::BrowserCollectionBasis::Consent,
            None,
            BrowserEventProperties::page_view("/cart", None, None).unwrap(),
        )
        .unwrap();
        assert_eq!(
            repository
                .append_browser_events(
                    &actor,
                    &[unconsented],
                    1,
                    BrowserCollectionMode::OptIn,
                    true,
                    now,
                )
                .await
                .unwrap(),
            1
        );

        let jobs = repository.claim_deliveries(10).await.unwrap();
        let job = jobs
            .iter()
            .find(|job| job.store_id == store_id)
            .expect("the consented event must create one Meta delivery");
        let competing = repository.claim_deliveries(10).await.unwrap();
        assert!(competing.iter().all(|job| job.store_id != store_id));
        repository
            .finish_delivery(
                job,
                Ok(AnalyticsDeliveryReceipt {
                    provider_reference: Some("trace".into()),
                }),
                now,
            )
            .await
            .unwrap();
        let status: String = sqlx::query_scalar(
            "SELECT delivery_status::text FROM integration.analytics_deliveries WHERE id=$1",
        )
        .bind(job.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "processed");

        let actor = StoreQueries::new(Arc::new(PostgresStoreReadRepository::new(pool.clone())))
            .authorize(user_id, store_id)
            .await
            .unwrap()
            .with_access_key(AccessKeyId::from_uuid(Uuid::now_v7()));
        let request = IdempotencyRequest {
            key: Uuid::now_v7().to_string(),
            request_fingerprint: [7; 32],
        };
        let settings = AnalyticsSettings::new(true, BrowserCollectionMode::OptOut, true).unwrap();
        let first = repository
            .update_settings(actor, store_id, settings, &request, now)
            .await
            .unwrap();
        let replay = repository
            .update_settings(
                actor,
                store_id,
                settings,
                &request,
                now + Duration::seconds(1),
            )
            .await
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(first.revision, 1);
        assert_eq!(
            first.settings.browser_collection_mode(),
            BrowserCollectionMode::OptOut
        );

        let shopper_id = Uuid::now_v7();
        let price_list_id = Uuid::now_v7();
        let cart_id = Uuid::now_v7();
        let checkout_id = Uuid::now_v7();
        let order_id = Uuid::now_v7();
        let provider_account_id = Uuid::now_v7();
        let payment_attempt_id = Uuid::now_v7();
        sqlx::query("INSERT INTO commerce.store_currencies(store_id,currency) VALUES($1,'USD')")
            .bind(store_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO commerce.price_lists(id,store_id,code,name,currency,status) VALUES($1,$2,'default','Default','USD','active')")
            .bind(price_list_id).bind(store_id.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.shoppers(id,store_id) VALUES($1,$2)")
            .bind(shopper_id)
            .bind(store_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO commerce.carts(id,store_id,sales_channel_id,shopper_id,price_list_id,currency) VALUES($1,$2,$3,$4,$5,'USD')")
            .bind(cart_id).bind(store_id.as_uuid()).bind(channel_id.as_uuid()).bind(shopper_id).bind(price_list_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.checkouts(id,store_id,cart_id,shopper_id,sales_channel_id,price_list_id,currency,subtotal_amount_minor,discount_amount_minor,tax_amount_minor,tax_inclusive,shipping_amount_minor,total_amount_minor,expires_at,status,closed_at) VALUES($1,$2,$3,$4,$5,$6,'USD',1000,0,0,false,0,1000,$7,'completed',$8)")
            .bind(checkout_id).bind(store_id.as_uuid()).bind(cart_id).bind(shopper_id).bind(channel_id.as_uuid()).bind(price_list_id).bind(now + Duration::hours(1)).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.orders(id,store_id,order_number,sales_channel_id,checkout_id,shopper_id,price_list_id,currency,subtotal_amount_minor,discount_amount_minor,tax_amount_minor,tax_inclusive,shipping_amount_minor,total_amount_minor,status) VALUES($1,$2,'W-20260820-TEST0001',$3,$4,$5,$6,'USD',1000,0,0,false,0,1000,'confirmed')")
            .bind(order_id).bind(store_id.as_uuid()).bind(channel_id.as_uuid()).bind(checkout_id).bind(shopper_id).bind(price_list_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.provider_accounts(id,store_id,provider,created_by_user_id) VALUES($1,$2,'sandbox',$3)")
            .bind(provider_account_id).bind(store_id.as_uuid()).bind(user_id.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.payment_attempts(id,store_id,order_id,shopper_id,provider_account_id,amount_minor,currency,status) VALUES($1,$2,$3,$4,$5,1000,'USD','captured')")
            .bind(payment_attempt_id).bind(store_id.as_uuid()).bind(order_id).bind(shopper_id).bind(provider_account_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO integration.analytics_events(id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,shopper_id,session_id,path,analytics_storage_consent,advertising_storage_consent,provider_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at) VALUES(uuidv7(),uuidv7(),$1,$2,'page_view','browser','consent',1,$3,uuidv7(),'/landing',true,false,false,'test-v1',1,$4,$5,$6)")
            .bind(store_id.as_uuid())
            .bind(channel_id.as_uuid())
            .bind(shopper_id)
            .bind(json!({"traffic":{"first":{"source":"meta"},"session":{"source":"meta"},"last_non_direct":{"source":"meta"}}}))
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();

        let first_capture = ServerCommerceEventJob {
            attempts: 1,
            id: Uuid::now_v7(),
            store_id,
            event_type: "analytics.payment.captured".into(),
            aggregate_id: payment_attempt_id,
            payload: json!({"payment_attempt_id":payment_attempt_id}),
            occurred_at: now,
        };
        repository
            .ingest_server_event(&first_capture, now)
            .await
            .unwrap();
        let purchase_event_id: Uuid = sqlx::query_scalar(
            "SELECT event_id FROM integration.analytics_events WHERE order_id=$1 AND event_name='purchase'",
        )
        .bind(order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(purchase_event_id, order_id);
        let first: (bool, Uuid, Option<String>) = sqlx::query_as(
            "SELECT provider_eligible,shopper_id,properties#>>'{traffic,session,source}' FROM integration.analytics_events WHERE event_id=$1",
        )
        .bind(order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(first, (true, shopper_id, None));

        let replayed_capture = ServerCommerceEventJob {
            attempts: 1,
            id: Uuid::now_v7(),
            ..first_capture.clone()
        };
        repository
            .ingest_server_event(&replayed_capture, now + Duration::seconds(1))
            .await
            .unwrap();
        let purchase_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM integration.analytics_events WHERE order_id=$1 AND event_name='purchase'",
        )
        .bind(order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(purchase_count, 1);

        let payment_info = ServerCommerceEventJob {
            attempts: 1,
            id: Uuid::now_v7(),
            event_type: "analytics.payment.initiated".into(),
            ..first_capture
        };
        repository
            .ingest_server_event(&payment_info, now + Duration::seconds(2))
            .await
            .unwrap();
        let payment_info_event_id: Uuid = sqlx::query_scalar(
            "SELECT event_id FROM integration.analytics_events WHERE payment_attempt_id=$1 AND event_name='add_payment_info'",
        )
        .bind(payment_attempt_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(payment_info_event_id, payment_attempt_id);
    }
}
