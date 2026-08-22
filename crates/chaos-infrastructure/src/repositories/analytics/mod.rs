use async_trait::async_trait;
use chaos_application::{ApplicationError, ports::*, store::StoreActor};
use chaos_domain::store::StoreId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::shared::idempotency::{self, IdempotencyScope};

const CONFIGURE_DESTINATION_OPERATION: &str = "analytics.configure_destination";

pub struct PostgresAnalyticsEventStore {
    pool: PgPool,
}

pub struct PostgresAnalyticsDestinationStore {
    pool: PgPool,
}

pub struct PostgresAnalyticsDeliveryStore {
    pool: PgPool,
}

impl PostgresAnalyticsEventStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl PostgresAnalyticsDestinationStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl PostgresAnalyticsDeliveryStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
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

/// Append one behavior event. Commerce repositories use the same primitive so
/// server-side events are written in the business transaction that produced
/// them; analytics no longer needs a second outbox-to-ledger conversion.
pub(crate) struct AnalyticsEventToAppend {
    pub(crate) store_id: Uuid,
    pub(crate) shopper_id: Uuid,
    pub(crate) event_id: Uuid,
    pub(crate) event_name: String,
    pub(crate) properties: Value,
    pub(crate) occurred_at: OffsetDateTime,
    pub(crate) received_at: OffsetDateTime,
}

pub(crate) async fn append_event(
    tx: &mut Transaction<'_, Postgres>,
    event: AnalyticsEventToAppend,
) -> Result<bool, ApplicationError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':' || $2::text, 0))")
        .bind(event.store_id)
        .bind(event.event_id)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM integration.analytics_events WHERE store_id=$1 AND event_id=$2)",
    )
    .bind(event.store_id)
    .bind(event.event_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(db)?;
    if duplicate {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO integration.analytics_events
            (id,event_id,store_id,shopper_id,event_name,properties,occurred_at,received_at)
         VALUES (uuidv7(),$1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(event.event_id)
    .bind(event.store_id)
    .bind(event.shopper_id)
    .bind(event.event_name)
    .bind(event.properties)
    .bind(event.occurred_at)
    .bind(event.received_at)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok(true)
}

#[async_trait]
impl AnalyticsEventRepository for PostgresAnalyticsEventStore {
    async fn append_events(
        &self,
        actor: &MachineActor,
        shopper_id: Uuid,
        events: &[AnalyticsEventInput],
        received_at: OffsetDateTime,
    ) -> Result<usize, ApplicationError> {
        let channel = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, actor.store_id.as_uuid(), None).await?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM commerce.stores s
                JOIN commerce.sales_channels c ON c.store_id=s.id
                WHERE s.id=$1 AND c.id=$2 AND s.status='active' AND c.status='active'
            )",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if !active {
            return Err(ApplicationError::Forbidden);
        }
        let mut stored = 0;
        for event in events {
            let mut properties = event.properties.clone();
            if let Some(object) = properties.as_object_mut() {
                object.insert("_source".into(), Value::String("browser".into()));
            }
            if append_event(
                &mut tx,
                AnalyticsEventToAppend {
                    store_id: actor.store_id.as_uuid(),
                    shopper_id,
                    event_id: event.event_id,
                    event_name: event.event_name.clone(),
                    properties,
                    occurred_at: event.occurred_at,
                    received_at,
                },
            )
            .await?
            {
                stored += 1;
            }
        }
        tx.commit().await.map_err(db)?;
        Ok(stored)
    }
}

#[async_trait]
impl AnalyticsEventQueryRepository for PostgresAnalyticsEventStore {
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
            Uuid,
            OffsetDateTime,
            OffsetDateTime,
            Value,
            sqlx::types::Json<Vec<DeliverySnapshot>>,
        )> = sqlx::query_as(
            "SELECT e.id,e.event_id,e.event_name,e.shopper_id,e.occurred_at,e.received_at,e.properties,
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
               AND ($4::text IS NULL OR e.event_name=$4)
               AND ($5::text IS NULL OR e.properties->>'_source'=$5)
               AND ($6::text IS NULL OR EXISTS (
                   SELECT 1 FROM integration.analytics_deliveries filter_delivery
                    WHERE filter_delivery.store_id=e.store_id
                      AND filter_delivery.analytics_event_id=e.id
                      AND filter_delivery.delivery_status::text=$6
               ))
               AND ($7::uuid IS NULL OR e.shopper_id=$7)
             GROUP BY e.id,e.event_id,e.event_name,e.shopper_id,e.occurred_at,e.received_at,e.properties
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
                    shopper_id,
                    occurred_at,
                    received_at,
                    properties,
                    deliveries,
                )| {
                    AnalyticsEventRecord {
                        id,
                        event_id,
                        event_name,
                        shopper_id,
                        occurred_at,
                        received_at,
                        properties,
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
                    }
                },
            )
            .collect();
        Ok(AnalyticsEventPage { events, has_more })
    }
}

#[async_trait]
impl AnalyticsDestinationRepository for PostgresAnalyticsDestinationStore {
    async fn get_destination(
        &self,
        actor: StoreActor,
        store: StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsDestination>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let row: Option<(
            Uuid,
            String,
            String,
            String,
            Value,
            bool,
            OffsetDateTime,
            OffsetDateTime,
        )> = sqlx::query_as(
            "SELECT id,provider,external_account_reference,credential_secret_reference,
                        configuration,enabled,created_at,updated_at
                   FROM integration.analytics_destinations
                  WHERE store_id=$1 AND provider=$2",
        )
        .bind(store.as_uuid())
        .bind(provider)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(row.map(|row| AnalyticsDestination {
            id: row.0,
            store_id: store,
            provider: row.1,
            external_account_reference: row.2,
            enabled: row.5,
            credentials_configured: true,
            configuration: row.4,
            created_at: row.6,
            updated_at: row.7,
        }))
    }

    async fn configure_destination(
        &self,
        actor: StoreActor,
        store: StoreId,
        configuration: AnalyticsDestinationConfiguration,
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
        let row: (
            Uuid,
            String,
            String,
            Value,
            bool,
            OffsetDateTime,
            OffsetDateTime,
        ) = sqlx::query_as(
            "SELECT destination_id,destination_provider,destination_external_account_reference,\
                        destination_configuration,destination_enabled,destination_created_at,\
                        destination_updated_at \
                   FROM integration.configure_analytics_destination($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(store.as_uuid())
        .bind(configuration.provider)
        .bind(configuration.external_account_reference)
        .bind(configuration.credential_secret_reference)
        .bind(configuration.configuration)
        .bind(configuration.enabled)
        .bind(actor.user_id().as_uuid())
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        let result = AnalyticsDestination {
            id: row.0,
            store_id: store,
            provider: row.1,
            external_account_reference: row.2,
            enabled: row.4,
            credentials_configured: true,
            configuration: row.3,
            created_at: row.5,
            updated_at: row.6,
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
impl AnalyticsDeliveryRepository for PostgresAnalyticsDeliveryStore {
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
        let rows: Vec<(Uuid, Uuid, Uuid, Uuid, i32)> = sqlx::query_as(
            "SELECT id,store_id,destination_id,analytics_event_id,attempts
               FROM integration.claim_analytics_deliveries($1)",
        )
        .bind(i32::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|row| AnalyticsDeliveryJob {
                id: row.0,
                store_id: StoreId::from_uuid(row.1),
                destination_id: row.2,
                analytics_event_id: row.3,
                attempts: u32::try_from(row.4).unwrap_or(u32::MAX),
            })
            .collect())
    }

    async fn load_delivery(
        &self,
        job: &AnalyticsDeliveryJob,
    ) -> Result<AnalyticsDeliveryCommand, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, job.store_id.as_uuid(), None).await?;
        let row: (Uuid, String, String, String, Value, String, OffsetDateTime, Uuid, Value) =
            sqlx::query_as(
                "SELECT e.event_id,destination.provider,destination.external_account_reference,
                        destination.credential_secret_reference,destination.configuration,
                        e.event_name,e.occurred_at,e.shopper_id,e.properties
                   FROM integration.analytics_deliveries delivery
                   JOIN integration.analytics_events e
                     ON e.store_id=delivery.store_id AND e.id=delivery.analytics_event_id
                   JOIN integration.analytics_destinations destination
                     ON destination.store_id=delivery.store_id AND destination.id=delivery.destination_id
                  WHERE delivery.store_id=$1 AND delivery.id=$2
                    AND delivery.delivery_status='pending' AND destination.enabled",
            )
            .bind(job.store_id.as_uuid())
            .bind(job.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(AnalyticsDeliveryCommand {
            delivery_id: job.id,
            provider: row.1,
            event_id: row.0,
            external_account_reference: row.2,
            credential_secret_reference: row.3,
            configuration: row.4,
            event_name: row.5,
            occurred_at: row.6,
            shopper_id: row.7,
            properties: row.8,
        })
    }

    async fn finish_delivery(
        &self,
        job: &AnalyticsDeliveryJob,
        result: Result<AnalyticsDeliveryReceipt, AnalyticsDeliveryError>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (succeeded, reference, error, retryable) = match result {
            Ok(receipt) => (true, receipt.provider_reference, None, false),
            Err(error) => (false, None, Some(error.message), error.retryable),
        };
        let finished: Option<bool> = sqlx::query_scalar(
            "SELECT integration.finish_analytics_event_delivery($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(job.id)
        .bind(i32::try_from(job.attempts).unwrap_or(i32::MAX))
        .bind(succeeded)
        .bind(retryable)
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

fn destination_snapshot(item: &AnalyticsDestination) -> Result<Value, ApplicationError> {
    serde_json::to_value(DestinationSnapshot {
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
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn destination_from_snapshot(value: Value) -> Result<AnalyticsDestination, ApplicationError> {
    let item: DestinationSnapshot =
        serde_json::from_value(value).map_err(|_| invalid_snapshot())?;
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

fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("invalid Analytics idempotency snapshot"))
}

fn db(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
