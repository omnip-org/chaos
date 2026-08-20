use async_trait::async_trait;
use chaos_application::{ApplicationError, ports::*, store::StoreActor};
use chaos_domain::{
    analytics::{
        AnalyticsSettings, BrowserCollectionBasis, BrowserCollectionMode, BrowserEvent,
        BrowserEventProperties, TrafficAttribution, TrafficTouchpoint,
    },
    identity::UserId,
    sales::CustomerId,
    store::StoreId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const UPDATE_SETTINGS_OPERATION: &str = "analytics.update_settings";
const LINK_VISITOR_OPERATION: &str = "analytics.link_visitor";
const REQUEST_ERASURE_OPERATION: &str = "analytics.request_erasure";
const CONFIGURE_META_OPERATION: &str = "analytics.configure_meta";

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
    meta_reporting_enabled: bool,
    identity_linking_enabled: bool,
    raw_event_retention_days: i16,
    updated_by: Uuid,
    updated_at: OffsetDateTime,
}
#[derive(FromRow)]
struct ErasureRow {
    id: Uuid,
    store_id: Uuid,
    visitor_id: Option<Uuid>,
    customer_id: Option<Uuid>,
    status: String,
    requested_by: Uuid,
    commerce_events_deleted: i64,
    visitor_links_deleted: i64,
    requested_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
}

#[derive(Deserialize, Serialize)]
struct SettingsSnapshot {
    store_id: Uuid,
    revision: i32,
    collection_enabled: bool,
    browser_collection_mode: String,
    meta_reporting_enabled: bool,
    identity_linking_enabled: bool,
    raw_event_retention_days: u16,
    updated_by: Option<Uuid>,
    updated_at: Option<OffsetDateTime>,
}

#[derive(Deserialize, Serialize)]
struct VisitorLinkSnapshot {
    id: Uuid,
    store_id: Uuid,
    visitor_id: Uuid,
    customer_id: Uuid,
    consent_policy_version: String,
    advertising_storage_consent: bool,
    collection_basis: String,
    settings_revision: i32,
    linked_at: OffsetDateTime,
    retention_expires_at: OffsetDateTime,
}

#[derive(Deserialize, Serialize)]
struct MetaSnapshot {
    store_id: Uuid,
    dataset_id: String,
    capi_enabled: bool,
    credentials_configured: bool,
    test_event_code_configured: bool,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(Deserialize, Serialize)]
struct ErasureSnapshot {
    id: Uuid,
    store_id: Uuid,
    selector_kind: String,
    selector_id: Uuid,
    status: String,
    requested_by: Uuid,
    commerce_events_deleted: u64,
    visitor_links_deleted: u64,
    requested_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
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
        r.meta_reporting_enabled,
        r.identity_linking_enabled,
        u16::try_from(r.raw_event_retention_days).map_err(convert)?,
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
    sqlx::query_as("SELECT store_id,revision,collection_enabled,browser_collection_mode::text,meta_reporting_enabled,identity_linking_enabled,raw_event_retention_days,updated_by,updated_at FROM integration.analytics_settings WHERE store_id=$1").bind(store.as_uuid()).fetch_optional(&mut **tx).await.map_err(db)
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
        meta_enabled: bool,
        received: OffsetDateTime,
        expires: OffsetDateTime,
    ) -> Result<usize, ApplicationError> {
        let channel = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, actor.store_id.as_uuid(), None).await?;
        let capi:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM integration.meta_connections WHERE store_id=$1 AND capi_enabled)").bind(actor.store_id.as_uuid()).fetch_one(&mut *tx).await.map_err(db)?;
        let mut count = 0;
        for event in events {
            let mut columns = browser_columns(event.properties());
            if let Some(traffic) = event.traffic() {
                columns.properties["traffic"] = traffic_json(traffic);
            }
            let eligible = meta_enabled
                && capi
                && (event.consent().advertising_storage()
                    || (event.collection_basis()
                        == chaos_domain::analytics::BrowserCollectionBasis::StorePolicy
                        && browser_collection_mode == BrowserCollectionMode::OptOut));
            let id:Option<Uuid>=sqlx::query_scalar("INSERT INTO integration.commerce_events (id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,visitor_id,session_id,product_id,product_variant_id,cart_id,checkout_id,path,analytics_storage_consent,advertising_storage_consent,meta_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at,retention_expires_at) VALUES(uuidv7(),$1,$2,$3,$4::integration.commerce_event_name,'browser',$5::integration.browser_collection_basis,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22) ON CONFLICT(store_id,event_id) DO NOTHING RETURNING id")
    .bind(event.event_id()).bind(actor.store_id.as_uuid()).bind(channel.as_uuid()).bind(event.name().as_str()).bind(event.collection_basis().as_str()).bind(i16::try_from(event.schema_version()).map_err(convert)?).bind(event.visitor_id()).bind(event.session_id()).bind(columns.product_id).bind(columns.product_variant_id).bind(columns.cart_id).bind(columns.checkout_id).bind(columns.path).bind(event.consent().analytics_storage()).bind(event.consent().advertising_storage()).bind(eligible).bind(event.consent().policy_version()).bind(revision).bind(columns.properties).bind(event.occurred_at()).bind(received).bind(expires).fetch_optional(&mut *tx).await.map_err(db)?;
            if let Some(id) = id {
                count += 1;
                if eligible {
                    enqueue_meta(&mut tx, actor.store_id, id, received).await?
                }
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
        let r:SettingsRow=sqlx::query_as("INSERT INTO integration.analytics_settings(store_id,revision,collection_enabled,browser_collection_mode,meta_reporting_enabled,identity_linking_enabled,raw_event_retention_days,updated_by,updated_at) VALUES($1,1,$2,$3::integration.browser_collection_mode,$4,$5,$6,$7,$8) ON CONFLICT(store_id) DO UPDATE SET revision=integration.analytics_settings.revision+1,collection_enabled=EXCLUDED.collection_enabled,browser_collection_mode=EXCLUDED.browser_collection_mode,meta_reporting_enabled=EXCLUDED.meta_reporting_enabled,identity_linking_enabled=EXCLUDED.identity_linking_enabled,raw_event_retention_days=EXCLUDED.raw_event_retention_days,updated_by=EXCLUDED.updated_by,updated_at=EXCLUDED.updated_at RETURNING store_id,revision,collection_enabled,browser_collection_mode::text,meta_reporting_enabled,identity_linking_enabled,raw_event_retention_days,updated_by,updated_at").bind(store.as_uuid()).bind(p.collection_enabled()).bind(p.browser_collection_mode().as_str()).bind(p.meta_reporting_enabled()).bind(p.identity_linking_enabled()).bind(i16::try_from(p.raw_event_retention_days()).map_err(convert)?).bind(actor.user_id().as_uuid()).bind(now).fetch_one(&mut *tx).await.map_err(db)?;
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
impl AnalyticsPrivacyRepository for PostgresAnalyticsEventRepository {
    async fn link_visitor_to_customer(
        &self,
        actor: &CustomerActor,
        visitor: Uuid,
        consent: &str,
        advertising_consent: bool,
        collection_basis: BrowserCollectionBasis,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<VisitorCustomerLink, ApplicationError> {
        let channel = actor
            .machine
            .sales_channel_id
            .ok_or(ApplicationError::Forbidden)?;
        let store = actor.machine.store_id;
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), None).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            LINK_VISITOR_OPERATION,
            request,
        )
        .await?
        {
            let result = visitor_link_from_snapshot(snapshot)?;
            tx.commit().await.map_err(db)?;
            return Ok(result);
        }
        let r:(Uuid,i32,i16)=sqlx::query_as("SELECT c.id,COALESCE(s.revision,1),COALESCE(s.raw_event_retention_days,30::smallint) FROM commerce.customers c LEFT JOIN integration.analytics_settings s ON s.store_id=c.store_id WHERE c.store_id=$1 AND c.sales_channel_id=$2 AND c.user_id=$3 AND COALESCE(s.identity_linking_enabled,false) AND ($4::integration.browser_collection_basis='consent' OR ($4='store_policy' AND COALESCE(s.browser_collection_mode='opt_out',true)))").bind(store.as_uuid()).bind(channel.as_uuid()).bind(actor.user_id.as_uuid()).bind(collection_basis.as_str()).fetch_optional(&mut *tx).await.map_err(db)?.ok_or(ApplicationError::Forbidden)?;
        let expires = now + Duration::days(i64::from(r.2));
        let saved:(Uuid,OffsetDateTime,OffsetDateTime)=sqlx::query_as("INSERT INTO integration.visitor_customer_links(id,store_id,visitor_id,customer_id,consent_policy_version,advertising_storage_consent,collection_basis,settings_revision,linked_at,retention_expires_at) VALUES(uuidv7(),$1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(store_id,visitor_id,customer_id) DO UPDATE SET consent_policy_version=EXCLUDED.consent_policy_version,advertising_storage_consent=EXCLUDED.advertising_storage_consent,collection_basis=EXCLUDED.collection_basis,settings_revision=EXCLUDED.settings_revision,linked_at=EXCLUDED.linked_at,retention_expires_at=EXCLUDED.retention_expires_at RETURNING id,linked_at,retention_expires_at").bind(store.as_uuid()).bind(visitor).bind(r.0).bind(consent).bind(advertising_consent).bind(collection_basis.as_str()).bind(r.1).bind(now).bind(expires).fetch_one(&mut *tx).await.map_err(db)?;
        let result = VisitorCustomerLink {
            id: saved.0,
            store_id: store,
            visitor_id: visitor,
            customer_id: CustomerId::from_uuid(r.0),
            consent_policy_version: consent.into(),
            advertising_storage_consent: advertising_consent,
            collection_basis,
            settings_revision: r.1,
            linked_at: saved.1,
            retention_expires_at: saved.2,
        };
        idempotency::complete(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            LINK_VISITOR_OPERATION,
            request,
            201,
            visitor_link_snapshot(&result)?,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
    async fn request_erasure(
        &self,
        actor: StoreActor,
        store: StoreId,
        selector: AnalyticsErasureSelector,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<AnalyticsErasureRequest, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            REQUEST_ERASURE_OPERATION,
            request,
        )
        .await?
        {
            let result = erasure_from_snapshot(snapshot)?;
            tx.commit().await.map_err(db)?;
            return Ok(result);
        }
        let (v, c) = match selector {
            AnalyticsErasureSelector::Visitor(id) => (Some(id), None),
            AnalyticsErasureSelector::Customer(id) => (None, Some(id.as_uuid())),
        };
        let r:ErasureRow=sqlx::query_as("INSERT INTO integration.analytics_erasure_requests(id,store_id,visitor_id,customer_id,requested_by,requested_at) VALUES(uuidv7(),$1,$2,$3,$4,$5) RETURNING id,store_id,visitor_id,customer_id,status::text,requested_by,commerce_events_deleted,visitor_links_deleted,requested_at,completed_at").bind(store.as_uuid()).bind(v).bind(c).bind(actor.user_id().as_uuid()).bind(now).fetch_one(&mut *tx).await.map_err(db)?;
        let result = map_erasure(r)?;
        idempotency::complete(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            REQUEST_ERASURE_OPERATION,
            request,
            202,
            erasure_snapshot(&result)?,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
    async fn get_erasure_request(
        &self,
        actor: StoreActor,
        store: StoreId,
        id: Uuid,
    ) -> Result<Option<AnalyticsErasureRequest>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let r=sqlx::query_as::<_,ErasureRow>("SELECT id,store_id,visitor_id,customer_id,status::text,requested_by,commerce_events_deleted,visitor_links_deleted,requested_at,completed_at FROM integration.analytics_erasure_requests WHERE store_id=$1 AND id=$2").bind(store.as_uuid()).bind(id).fetch_optional(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        r.map(map_erasure).transpose()
    }
}

#[async_trait]
impl MetaConnectionRepository for PostgresAnalyticsEventRepository {
    async fn get_meta_connection(
        &self,
        actor: StoreActor,
        store: StoreId,
    ) -> Result<Option<MetaConnection>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let r:Option<(String,bool,bool,OffsetDateTime,OffsetDateTime)>=sqlx::query_as("SELECT dataset_id,capi_enabled,test_event_code IS NOT NULL,created_at,updated_at FROM integration.meta_connections WHERE store_id=$1").bind(store.as_uuid()).fetch_optional(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(r.map(|r| MetaConnection {
            store_id: store,
            dataset_id: r.0,
            capi_enabled: r.1,
            credentials_configured: true,
            test_event_code_configured: r.2,
            created_at: r.3,
            updated_at: r.4,
        }))
    }
    async fn configure_meta_connection(
        &self,
        actor: StoreActor,
        store: StoreId,
        c: MetaConnectionConfiguration,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<MetaConnection, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            CONFIGURE_META_OPERATION,
            request,
        )
        .await?
        {
            let result = meta_from_snapshot(snapshot)?;
            tx.commit().await.map_err(db)?;
            return Ok(result);
        }
        let test = c.test_event_code.is_some();
        let r:(String,bool,OffsetDateTime,OffsetDateTime)=sqlx::query_as("INSERT INTO integration.meta_connections(store_id,dataset_id,credential_secret_reference,test_event_code,capi_enabled,created_by,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$7) ON CONFLICT(store_id) DO UPDATE SET dataset_id=EXCLUDED.dataset_id,credential_secret_reference=EXCLUDED.credential_secret_reference,test_event_code=EXCLUDED.test_event_code,capi_enabled=EXCLUDED.capi_enabled,updated_at=EXCLUDED.updated_at RETURNING dataset_id,capi_enabled,created_at,updated_at").bind(store.as_uuid()).bind(c.dataset_id).bind(c.credential_secret_reference).bind(c.test_event_code).bind(c.capi_enabled).bind(actor.user_id().as_uuid()).bind(now).fetch_one(&mut *tx).await.map_err(db)?;
        let result = MetaConnection {
            store_id: store,
            dataset_id: r.0,
            capi_enabled: r.1,
            credentials_configured: true,
            test_event_code_configured: test,
            created_at: r.2,
            updated_at: r.3,
        };
        idempotency::complete(
            &mut tx,
            &IdempotencyScope::Store(store.as_uuid()),
            CONFIGURE_META_OPERATION,
            request,
            200,
            meta_snapshot(&result)?,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
}

#[async_trait]
impl AnalyticsWorkerRepository for PostgresAnalyticsEventRepository {
    async fn claim_server_events(
        &self,
        worker: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale: OffsetDateTime,
    ) -> Result<Vec<ServerCommerceEventJob>, ApplicationError> {
        let rows:Vec<(Uuid,Uuid,String,Uuid,Value,OffsetDateTime)>=sqlx::query_as("SELECT id,store_id,event_type,aggregate_id,payload,occurred_at FROM integration.claim_analytics_events($1,$2,$3,$4)").bind(worker).bind(i32::from(limit)).bind(now).bind(stale).fetch_all(&self.pool).await.map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| ServerCommerceEventJob {
                id: r.0,
                store_id: StoreId::from_uuid(r.1),
                event_type: r.2,
                aggregate_id: r.3,
                payload: r.4,
                occurred_at: r.5,
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
        let days:i16=sqlx::query_scalar("SELECT COALESCE((SELECT raw_event_retention_days FROM integration.analytics_settings WHERE store_id=$1),30::smallint)").bind(job.store_id.as_uuid()).fetch_one(&mut *tx).await.map_err(db)?;
        let meta:bool=sqlx::query_scalar("SELECT COALESCE((SELECT meta_reporting_enabled FROM integration.analytics_settings WHERE store_id=$1),false) AND EXISTS(SELECT 1 FROM integration.meta_connections WHERE store_id=$1 AND capi_enabled)").bind(job.store_id.as_uuid()).fetch_one(&mut *tx).await.map_err(db)?;
        let id = insert_server(&mut tx, job, now, days, meta).await?;
        if let Some(id) = id.filter(|_| meta) {
            enqueue_meta(&mut tx, job.store_id, id, now).await?
        }
        tx.commit().await.map_err(db)?;
        Ok(())
    }
    async fn finish_server_event(
        &self,
        worker: Uuid,
        job: &ServerCommerceEventJob,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (ok, e) = result.map_or_else(|e| (false, e), |_| (true, String::new()));
        let done: Option<bool> =
            sqlx::query_scalar("SELECT integration.finish_outbox_event($1,$2,$3,$4,8,$5)")
                .bind(job.id)
                .bind(worker)
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
    async fn claim_meta_deliveries(
        &self,
        worker: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale: OffsetDateTime,
    ) -> Result<Vec<MetaDeliveryJob>, ApplicationError> {
        let r:Vec<(Uuid,Uuid,Uuid)>=sqlx::query_as("SELECT id,store_id,commerce_event_id FROM integration.claim_meta_event_deliveries($1,$2,$3,$4)").bind(worker).bind(i32::from(limit)).bind(now).bind(stale).fetch_all(&self.pool).await.map_err(db)?;
        Ok(r.into_iter()
            .map(|r| MetaDeliveryJob {
                id: r.0,
                store_id: StoreId::from_uuid(r.1),
                commerce_event_id: r.2,
            })
            .collect())
    }
    async fn load_meta_delivery(
        &self,
        job: &MetaDeliveryJob,
    ) -> Result<MetaDeliveryCommand, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, job.store_id.as_uuid(), None).await?;
        let r:(Uuid,String,String,Option<String>,String,OffsetDateTime,Option<Uuid>,Option<Uuid>,Option<String>,Option<i64>,Option<String>,Value)=sqlx::query_as("SELECT e.event_id,c.dataset_id,c.credential_secret_reference,c.test_event_code,e.event_name::text,e.occurred_at,e.visitor_id,e.customer_id,e.properties->>'source_url',e.value_minor,e.currency,e.properties || jsonb_strip_nulls(jsonb_build_object('content_ids',CASE WHEN e.product_variant_id IS NOT NULL THEN jsonb_build_array(e.product_variant_id::text) WHEN e.product_id IS NOT NULL THEN jsonb_build_array(e.product_id::text) END,'path',e.path,'order_id',e.order_id,'payment_attempt_id',e.payment_attempt_id,'refund_id',e.refund_id)) FROM integration.meta_event_deliveries d JOIN integration.commerce_events e ON e.store_id=d.store_id AND e.id=d.commerce_event_id JOIN integration.meta_connections c ON c.store_id=d.store_id WHERE d.store_id=$1 AND d.id=$2 AND d.delivery_status='processing' AND c.capi_enabled").bind(job.store_id.as_uuid()).bind(job.id).fetch_one(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(MetaDeliveryCommand {
            delivery_id: job.id,
            event_id: r.0,
            dataset_id: r.1,
            credential_secret_reference: r.2,
            test_event_code: r.3,
            event_name: r.4,
            occurred_at: r.5,
            visitor_id: r.6,
            customer_id: r.7.map(CustomerId::from_uuid),
            source_url: r.8,
            value_minor: r.9,
            currency: r.10,
            properties: r.11,
        })
    }
    async fn finish_meta_delivery(
        &self,
        worker: Uuid,
        job: &MetaDeliveryJob,
        result: Result<MetaDeliveryReceipt, MetaDeliveryError>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (ok, reference, error, retry) = match result {
            Ok(r) => (true, r.provider_reference, None, false),
            Err(e) => (false, None, Some(e.message), e.retryable),
        };
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, job.store_id.as_uuid(), None).await?;
        let n=sqlx::query("UPDATE integration.meta_event_deliveries SET delivery_status=CASE WHEN $3 THEN 'processed'::integration.queue_status WHEN $6 AND attempts<8 THEN 'pending'::integration.queue_status ELSE 'dead_letter'::integration.queue_status END,available_at=CASE WHEN $3 THEN available_at ELSE $5+make_interval(secs=>least(power(2,greatest(attempts-1,0))::integer,256)) END,locked_by=NULL,locked_at=NULL,delivered_at=CASE WHEN $3 THEN $5 ELSE NULL END,provider_reference=$4,last_error=$7,updated_at=$5 WHERE id=$1 AND store_id=$2 AND delivery_status='processing' AND locked_by=$8").bind(job.id).bind(job.store_id.as_uuid()).bind(ok).bind(reference).bind(now).bind(retry).bind(error).bind(worker).execute(&mut *tx).await.map_err(db)?.rows_affected();
        tx.commit().await.map_err(db)?;
        if n == 1 {
            Ok(())
        } else {
            Err(ApplicationError::Conflict {
                code: "meta_delivery_lease_lost",
                message: "the Meta delivery lease is no longer owned by this worker",
            })
        }
    }
    async fn purge_expired(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsRetentionResult, ApplicationError> {
        let r:(i64,i64)=sqlx::query_as("SELECT commerce_events_deleted,visitor_links_deleted FROM integration.purge_expired_analytics_data($1,$2)").bind(i32::from(limit)).bind(now).fetch_one(&self.pool).await.map_err(db)?;
        Ok(AnalyticsRetentionResult {
            commerce_events_deleted: u64::try_from(r.0).map_err(convert)?,
            visitor_links_deleted: u64::try_from(r.1).map_err(convert)?,
        })
    }
    async fn process_erasure_requests(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<AnalyticsErasureBatchResult, ApplicationError> {
        let r:(i64,i64,i64)=sqlx::query_as("SELECT requests_completed,commerce_events_deleted,visitor_links_deleted FROM integration.process_analytics_erasure_requests($1,$2)").bind(i32::from(limit)).bind(now).fetch_one(&self.pool).await.map_err(db)?;
        Ok(AnalyticsErasureBatchResult {
            requests_completed: u64::try_from(r.0).map_err(convert)?,
            commerce_events_deleted: u64::try_from(r.1).map_err(convert)?,
            visitor_links_deleted: u64::try_from(r.2).map_err(convert)?,
        })
    }
}

async fn enqueue_meta(
    tx: &mut Transaction<'_, Postgres>,
    store: StoreId,
    event: Uuid,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query("INSERT INTO integration.meta_event_deliveries(id,store_id,commerce_event_id,available_at,created_at,updated_at) VALUES(uuidv7(),$1,$2,$3,$3,$3) ON CONFLICT(store_id,commerce_event_id) DO NOTHING").bind(store.as_uuid()).bind(event).bind(now).execute(&mut **tx).await.map_err(db)?;
    Ok(())
}
async fn insert_server(
    tx: &mut Transaction<'_, Postgres>,
    job: &ServerCommerceEventJob,
    now: OffsetDateTime,
    days: i16,
    meta: bool,
) -> Result<Option<Uuid>, ApplicationError> {
    let name = match job.event_type.as_str() {
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
    let query = if name == "purchase" || name == "add_payment_info" {
        let expected_status = if name == "purchase" {
            "captured"
        } else {
            "any"
        };
        let query = "INSERT INTO integration.commerce_events(id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,visitor_id,customer_id,checkout_id,order_id,payment_attempt_id,value_minor,currency,analytics_storage_consent,advertising_storage_consent,meta_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at,retention_expires_at) SELECT uuidv7(),CASE WHEN $8='purchase' THEN o.id ELSE a.id END,a.store_id,o.sales_channel_id,$8::integration.commerce_event_name,'server','server',1,consent.visitor_id,o.customer_id,o.checkout_id,o.id,a.id,a.amount_minor,a.currency,true,COALESCE(consent.advertising_storage_consent,false),$4 AND (COALESCE(consent.advertising_storage_consent,false) OR COALESCE(s.browser_collection_mode='opt_out',true)),consent.consent_policy_version,COALESCE(s.revision,1),CASE WHEN consent.traffic IS NULL THEN '{}'::jsonb ELSE jsonb_build_object('traffic',consent.traffic) END,$2,$3,$3+make_interval(days=>$5) FROM commerce.payment_attempts a JOIN commerce.orders o ON o.store_id=a.store_id AND o.id=a.order_id LEFT JOIN integration.analytics_settings s ON s.store_id=a.store_id LEFT JOIN LATERAL (SELECT link.visitor_id,link.advertising_storage_consent,link.consent_policy_version,(SELECT event.properties->'traffic' FROM integration.commerce_events event WHERE event.store_id=link.store_id AND event.visitor_id=link.visitor_id AND event.properties ? 'traffic' ORDER BY event.occurred_at DESC,event.id DESC LIMIT 1) AS traffic FROM integration.visitor_customer_links link WHERE link.store_id=o.store_id AND link.customer_id=o.customer_id AND link.retention_expires_at>$3 ORDER BY link.linked_at DESC,link.id DESC LIMIT 1) consent ON true WHERE a.store_id=$6 AND a.id=$7 AND ($9 = 'any' OR a.status::text=$9) ON CONFLICT(store_id,event_id) DO NOTHING RETURNING id";
        return sqlx::query_scalar(query)
            .bind(job.id)
            .bind(job.occurred_at)
            .bind(now)
            .bind(meta)
            .bind(i32::from(days))
            .bind(job.store_id.as_uuid())
            .bind(job.aggregate_id)
            .bind(name)
            .bind(expected_status)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db);
    } else {
        "INSERT INTO integration.commerce_events(id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,visitor_id,customer_id,checkout_id,order_id,payment_attempt_id,refund_id,value_minor,currency,analytics_storage_consent,advertising_storage_consent,meta_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at,retention_expires_at) SELECT uuidv7(),$1,r.store_id,o.sales_channel_id,'refund','server','server',1,consent.visitor_id,o.customer_id,o.checkout_id,o.id,a.id,r.id,r.amount_minor,r.currency,true,COALESCE(consent.advertising_storage_consent,false),$4 AND (COALESCE(consent.advertising_storage_consent,false) OR COALESCE(s.browser_collection_mode='opt_out',true)),consent.consent_policy_version,COALESCE(s.revision,1),CASE WHEN consent.traffic IS NULL THEN '{}'::jsonb ELSE jsonb_build_object('traffic',consent.traffic) END,$2,$3,$3+make_interval(days=>$5) FROM commerce.refunds r JOIN commerce.payment_attempts a ON a.store_id=r.store_id AND a.id=r.payment_attempt_id JOIN commerce.orders o ON o.store_id=a.store_id AND o.id=a.order_id LEFT JOIN integration.analytics_settings s ON s.store_id=r.store_id LEFT JOIN LATERAL (SELECT link.visitor_id,link.advertising_storage_consent,link.consent_policy_version,(SELECT event.properties->'traffic' FROM integration.commerce_events event WHERE event.store_id=link.store_id AND event.visitor_id=link.visitor_id AND event.properties ? 'traffic' ORDER BY event.occurred_at DESC,event.id DESC LIMIT 1) AS traffic FROM integration.visitor_customer_links link WHERE link.store_id=o.store_id AND link.customer_id=o.customer_id AND link.retention_expires_at>$3 ORDER BY link.linked_at DESC,link.id DESC LIMIT 1) consent ON true WHERE r.store_id=$6 AND r.id=$7 AND r.status='succeeded' ON CONFLICT(store_id,event_id) DO NOTHING RETURNING id"
    };
    sqlx::query_scalar(query)
        .bind(job.id)
        .bind(job.occurred_at)
        .bind(now)
        .bind(meta)
        .bind(i32::from(days))
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
fn map_erasure(r: ErasureRow) -> Result<AnalyticsErasureRequest, ApplicationError> {
    let selector = match (r.visitor_id, r.customer_id) {
        (Some(id), None) => AnalyticsErasureSelector::Visitor(id),
        (None, Some(id)) => AnalyticsErasureSelector::Customer(CustomerId::from_uuid(id)),
        _ => {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "invalid erasure selector"
            )));
        }
    };
    Ok(AnalyticsErasureRequest {
        id: r.id,
        store_id: StoreId::from_uuid(r.store_id),
        selector,
        status: if r.status == "completed" {
            AnalyticsErasureStatus::Completed
        } else {
            AnalyticsErasureStatus::Pending
        },
        requested_by: UserId::from_uuid(r.requested_by),
        commerce_events_deleted: u64::try_from(r.commerce_events_deleted).map_err(convert)?,
        visitor_links_deleted: u64::try_from(r.visitor_links_deleted).map_err(convert)?,
        requested_at: r.requested_at,
        completed_at: r.completed_at,
    })
}

fn settings_snapshot(item: &StoreAnalyticsSettings) -> Result<Value, ApplicationError> {
    snapshot_value(SettingsSnapshot {
        store_id: item.store_id.as_uuid(),
        revision: item.revision,
        collection_enabled: item.settings.collection_enabled(),
        browser_collection_mode: item.settings.browser_collection_mode().as_str().into(),
        meta_reporting_enabled: item.settings.meta_reporting_enabled(),
        identity_linking_enabled: item.settings.identity_linking_enabled(),
        raw_event_retention_days: item.settings.raw_event_retention_days(),
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
            item.meta_reporting_enabled,
            item.identity_linking_enabled,
            item.raw_event_retention_days,
        )?,
        updated_by: item.updated_by.map(UserId::from_uuid),
        updated_at: item.updated_at,
    })
}

fn visitor_link_snapshot(item: &VisitorCustomerLink) -> Result<Value, ApplicationError> {
    snapshot_value(VisitorLinkSnapshot {
        id: item.id,
        store_id: item.store_id.as_uuid(),
        visitor_id: item.visitor_id,
        customer_id: item.customer_id.as_uuid(),
        consent_policy_version: item.consent_policy_version.clone(),
        advertising_storage_consent: item.advertising_storage_consent,
        collection_basis: item.collection_basis.as_str().into(),
        settings_revision: item.settings_revision,
        linked_at: item.linked_at,
        retention_expires_at: item.retention_expires_at,
    })
}

fn visitor_link_from_snapshot(value: Value) -> Result<VisitorCustomerLink, ApplicationError> {
    let item: VisitorLinkSnapshot = parse_snapshot(value)?;
    Ok(VisitorCustomerLink {
        id: item.id,
        store_id: StoreId::from_uuid(item.store_id),
        visitor_id: item.visitor_id,
        customer_id: CustomerId::from_uuid(item.customer_id),
        consent_policy_version: item.consent_policy_version,
        advertising_storage_consent: item.advertising_storage_consent,
        collection_basis: match item.collection_basis.as_str() {
            "consent" => BrowserCollectionBasis::Consent,
            "store_policy" => BrowserCollectionBasis::StorePolicy,
            value => {
                return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                    "invalid visitor link collection basis: {value}"
                )));
            }
        },
        settings_revision: item.settings_revision,
        linked_at: item.linked_at,
        retention_expires_at: item.retention_expires_at,
    })
}

fn meta_snapshot(item: &MetaConnection) -> Result<Value, ApplicationError> {
    snapshot_value(MetaSnapshot {
        store_id: item.store_id.as_uuid(),
        dataset_id: item.dataset_id.clone(),
        capi_enabled: item.capi_enabled,
        credentials_configured: item.credentials_configured,
        test_event_code_configured: item.test_event_code_configured,
        created_at: item.created_at,
        updated_at: item.updated_at,
    })
}

fn meta_from_snapshot(value: Value) -> Result<MetaConnection, ApplicationError> {
    let item: MetaSnapshot = parse_snapshot(value)?;
    Ok(MetaConnection {
        store_id: StoreId::from_uuid(item.store_id),
        dataset_id: item.dataset_id,
        capi_enabled: item.capi_enabled,
        credentials_configured: item.credentials_configured,
        test_event_code_configured: item.test_event_code_configured,
        created_at: item.created_at,
        updated_at: item.updated_at,
    })
}

fn erasure_snapshot(item: &AnalyticsErasureRequest) -> Result<Value, ApplicationError> {
    let (selector_kind, selector_id) = match item.selector {
        AnalyticsErasureSelector::Visitor(id) => ("visitor", id),
        AnalyticsErasureSelector::Customer(id) => ("customer", id.as_uuid()),
    };
    snapshot_value(ErasureSnapshot {
        id: item.id,
        store_id: item.store_id.as_uuid(),
        selector_kind: selector_kind.into(),
        selector_id,
        status: match item.status {
            AnalyticsErasureStatus::Pending => "pending",
            AnalyticsErasureStatus::Completed => "completed",
        }
        .into(),
        requested_by: item.requested_by.as_uuid(),
        commerce_events_deleted: item.commerce_events_deleted,
        visitor_links_deleted: item.visitor_links_deleted,
        requested_at: item.requested_at,
        completed_at: item.completed_at,
    })
}

fn erasure_from_snapshot(value: Value) -> Result<AnalyticsErasureRequest, ApplicationError> {
    let item: ErasureSnapshot = parse_snapshot(value)?;
    let selector = match item.selector_kind.as_str() {
        "visitor" => AnalyticsErasureSelector::Visitor(item.selector_id),
        "customer" => AnalyticsErasureSelector::Customer(CustomerId::from_uuid(item.selector_id)),
        _ => return Err(invalid_snapshot()),
    };
    let status = match item.status.as_str() {
        "pending" => AnalyticsErasureStatus::Pending,
        "completed" => AnalyticsErasureStatus::Completed,
        _ => return Err(invalid_snapshot()),
    };
    Ok(AnalyticsErasureRequest {
        id: item.id,
        store_id: StoreId::from_uuid(item.store_id),
        selector,
        status,
        requested_by: UserId::from_uuid(item.requested_by),
        commerce_events_deleted: item.commerce_events_deleted,
        visitor_links_deleted: item.visitor_links_deleted,
        requested_at: item.requested_at,
        completed_at: item.completed_at,
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

    use chaos_application::{
        ports::{
            AnalyticsEventRepository as _, AnalyticsSettingsRepository as _,
            AnalyticsWorkerRepository as _,
        },
        store::StoreQueries,
    };
    use chaos_domain::{
        analytics::{BrowserEvent, BrowserEventProperties, ConsentSnapshot},
        identity::{AccessKeyId, UserId},
        store::{PublishableKeyId, SalesChannelId, StoreId},
    };
    use sqlx::postgres::PgPoolOptions;

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
        sqlx::query("INSERT INTO integration.meta_connections(store_id,dataset_id,credential_secret_reference,capi_enabled,created_by,created_at,updated_at) VALUES($1,'12345','env://CHAOS_ANALYTICS_SECRET_TEST',true,$2,$3,$3)")
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
                now + Duration::days(30),
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
                    now + Duration::days(30),
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
                    now + Duration::days(30)
                )
                .await
                .unwrap(),
            1
        );

        let worker_a = Uuid::now_v7();
        let jobs = repository
            .claim_meta_deliveries(worker_a, 10, now, now - Duration::minutes(1))
            .await
            .unwrap();
        let job = jobs
            .iter()
            .find(|job| job.store_id == store_id)
            .expect("the consented event must create one Meta delivery");
        let competing = repository
            .claim_meta_deliveries(Uuid::now_v7(), 10, now, now - Duration::minutes(1))
            .await
            .unwrap();
        assert!(competing.iter().all(|job| job.store_id != store_id));
        repository
            .finish_meta_delivery(
                worker_a,
                job,
                Ok(MetaDeliveryReceipt {
                    provider_reference: Some("trace".into()),
                }),
                now,
            )
            .await
            .unwrap();
        let status: String = sqlx::query_scalar(
            "SELECT delivery_status::text FROM integration.meta_event_deliveries WHERE id=$1",
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
        let settings =
            AnalyticsSettings::new(true, BrowserCollectionMode::OptOut, true, true, 45).unwrap();
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

        let customer_id = Uuid::now_v7();
        let visitor_id = Uuid::now_v7();
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
        sqlx::query(
            "INSERT INTO commerce.customers(id,store_id,user_id,email) VALUES($1,$2,$3,$4)",
        )
        .bind(customer_id)
        .bind(store_id.as_uuid())
        .bind(user_id.as_uuid())
        .bind(format!("customer-{}@example.com", customer_id.simple()))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO commerce.carts(id,store_id,sales_channel_id,shopper_id,customer_id,price_list_id,currency) VALUES($1,$2,$3,$4,$5,$6,'USD')")
            .bind(cart_id).bind(store_id.as_uuid()).bind(channel_id.as_uuid()).bind(shopper_id).bind(customer_id).bind(price_list_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.checkouts(id,store_id,cart_id,shopper_id,customer_id,sales_channel_id,price_list_id,currency,subtotal_amount_minor,discount_amount_minor,tax_amount_minor,tax_inclusive,shipping_amount_minor,total_amount_minor,expires_at,status,closed_at) VALUES($1,$2,$3,$4,$5,$6,$7,'USD',1000,0,0,false,0,1000,$8,'completed',$9)")
            .bind(checkout_id).bind(store_id.as_uuid()).bind(cart_id).bind(shopper_id).bind(customer_id).bind(channel_id.as_uuid()).bind(price_list_id).bind(now + Duration::hours(1)).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.orders(id,store_id,order_number,sales_channel_id,checkout_id,shopper_id,customer_id,price_list_id,currency,subtotal_amount_minor,discount_amount_minor,tax_amount_minor,tax_inclusive,shipping_amount_minor,total_amount_minor,status) VALUES($1,$2,'W-20260820-TEST0001',$3,$4,$5,$6,$7,'USD',1000,0,0,false,0,1000,'confirmed')")
            .bind(order_id).bind(store_id.as_uuid()).bind(channel_id.as_uuid()).bind(checkout_id).bind(shopper_id).bind(customer_id).bind(price_list_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.provider_accounts(id,store_id,provider,external_account_reference,created_by_user_id) VALUES($1,$2,'sandbox',$3,$4)")
            .bind(provider_account_id).bind(store_id.as_uuid()).bind(provider_account_id.to_string()).bind(user_id.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO commerce.payment_attempts(id,store_id,order_id,shopper_id,provider_account_id,amount_minor,currency,status) VALUES($1,$2,$3,$4,$5,1000,'USD','captured')")
            .bind(payment_attempt_id).bind(store_id.as_uuid()).bind(order_id).bind(shopper_id).bind(provider_account_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO integration.visitor_customer_links(id,store_id,visitor_id,customer_id,consent_policy_version,advertising_storage_consent,collection_basis,settings_revision,linked_at,retention_expires_at) VALUES(uuidv7(),$1,$2,$3,'test-v1',false,'store_policy',1,$4,$5)")
            .bind(store_id.as_uuid()).bind(visitor_id).bind(customer_id).bind(now).bind(now + Duration::days(30)).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO integration.commerce_events(id,event_id,store_id,sales_channel_id,event_name,source,collection_basis,schema_version,visitor_id,session_id,path,analytics_storage_consent,advertising_storage_consent,meta_eligible,consent_policy_version,settings_revision,properties,occurred_at,received_at,retention_expires_at) VALUES(uuidv7(),uuidv7(),$1,$2,'page_view','browser','consent',1,$3,uuidv7(),'/landing',true,false,false,'test-v1',1,$4,$5,$5,$6)")
            .bind(store_id.as_uuid())
            .bind(channel_id.as_uuid())
            .bind(visitor_id)
            .bind(json!({"traffic":{"first":{"source":"meta"},"session":{"source":"meta"},"last_non_direct":{"source":"meta"}}}))
            .bind(now)
            .bind(now + Duration::days(30))
            .execute(&pool)
            .await
            .unwrap();

        let first_capture = ServerCommerceEventJob {
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
            "SELECT event_id FROM integration.commerce_events WHERE order_id=$1 AND event_name='purchase'",
        )
        .bind(order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(purchase_event_id, order_id);
        let first: (bool, Option<Uuid>, Option<String>) = sqlx::query_as(
            "SELECT meta_eligible,visitor_id,properties#>>'{traffic,session,source}' FROM integration.commerce_events WHERE event_id=$1",
        )
        .bind(order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(first, (true, Some(visitor_id), Some("meta".into())));

        let replayed_capture = ServerCommerceEventJob {
            id: Uuid::now_v7(),
            ..first_capture.clone()
        };
        repository
            .ingest_server_event(&replayed_capture, now + Duration::seconds(1))
            .await
            .unwrap();
        let purchase_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM integration.commerce_events WHERE order_id=$1 AND event_name='purchase'",
        )
        .bind(order_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(purchase_count, 1);

        let payment_info = ServerCommerceEventJob {
            id: Uuid::now_v7(),
            event_type: "analytics.payment.initiated".into(),
            ..first_capture
        };
        repository
            .ingest_server_event(&payment_info, now + Duration::seconds(2))
            .await
            .unwrap();
        let payment_info_event_id: Uuid = sqlx::query_scalar(
            "SELECT event_id FROM integration.commerce_events WHERE payment_attempt_id=$1 AND event_name='add_payment_info'",
        )
        .bind(payment_attempt_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(payment_info_event_id, payment_attempt_id);
    }
}
