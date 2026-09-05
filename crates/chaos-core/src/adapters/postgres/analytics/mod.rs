use crate::{ApplicationError, contracts::*, store::StoreActor};
use chaos_domain::store::StoreId;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

const MAX_UTM_VALUE_BYTES: usize = 2_048;

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

#[derive(serde::Deserialize)]
struct DeliverySnapshot {
    provider: String,
    status: String,
    // jsonb_build_object serializes PostgreSQL timestamps as RFC3339 strings.
    delivered_at: Option<String>,
    provider_reference: Option<String>,
    last_error: Option<String>,
}

type AnalyticsEventRow = (
    Uuid,
    Uuid,
    String,
    String,
    Uuid,
    Uuid,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    OffsetDateTime,
    OffsetDateTime,
    Value,
    sqlx::types::Json<Vec<DeliverySnapshot>>,
);

type AnalyticsDestinationRow = (
    Uuid,
    String,
    String,
    String,
    Value,
    bool,
    OffsetDateTime,
    OffsetDateTime,
);

async fn context(
    tx: &mut Transaction<'_, Postgres>,
    store: Uuid,
    user: Option<Uuid>,
) -> Result<(), ApplicationError> {
    crate::adapters::postgres::database::set_store_context(
        tx,
        chaos_domain::store::StoreId::from_uuid(store),
    )
    .await
    .map_err(db)?;
    crate::adapters::postgres::database::set_optional_user_context(
        tx,
        user.map(chaos_domain::identity::UserId::from_uuid),
    )
    .await
    .map_err(db)
}

/// Append one behavior event. Commerce repositories use the same primitive so
/// server-side events are written in the business transaction that produced
/// them; delivery rows are scheduled asynchronously so provider queues cannot
/// roll back event collection or a commerce transaction.
pub(crate) struct AnalyticsEventToAppend {
    pub(crate) store_id: Uuid,
    pub(crate) channel_id: Uuid,
    pub(crate) shopper_id: Uuid,
    pub(crate) event_id: Uuid,
    pub(crate) event_name: String,
    pub(crate) event_source: &'static str,
    pub(crate) properties: Value,
    pub(crate) occurred_at: OffsetDateTime,
    pub(crate) received_at: OffsetDateTime,
}

pub(crate) async fn append_event(
    tx: &mut Transaction<'_, Postgres>,
    event: AnalyticsEventToAppend,
) -> Result<bool, ApplicationError> {
    let mut properties = event.properties;
    let session_id = properties
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let direct_utm_source = normalized_utm_value(properties.get("utm_source"));
    let direct_utm_medium = normalized_utm_value(properties.get("utm_medium"));
    let direct_utm_campaign = normalized_utm_value(properties.get("utm_campaign"));
    let direct_utm_term = normalized_utm_value(properties.get("utm_term"));
    let direct_utm_content = normalized_utm_value(properties.get("utm_content"));
    let utm_source = direct_utm_source
        .clone()
        .or_else(|| traffic_utm_value(&properties, "source"));
    let utm_medium = direct_utm_medium
        .clone()
        .or_else(|| traffic_utm_value(&properties, "medium"));
    let utm_campaign = direct_utm_campaign
        .clone()
        .or_else(|| traffic_utm_value(&properties, "campaign"));
    let utm_term = direct_utm_term
        .clone()
        .or_else(|| traffic_utm_value(&properties, "term"));
    let utm_content = direct_utm_content
        .clone()
        .or_else(|| traffic_utm_value(&properties, "content"));
    if let Some(object) = properties.as_object_mut() {
        object.remove("session_id");
        for (key, value) in [
            ("utm_source", &direct_utm_source),
            ("utm_medium", &direct_utm_medium),
            ("utm_campaign", &direct_utm_campaign),
            ("utm_term", &direct_utm_term),
            ("utm_content", &direct_utm_content),
        ] {
            if value.is_some() {
                object.remove(key);
            }
        }
    }
    let analytics_event_id = Uuid::now_v7();
    let inserted_key: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO integration.analytics_event_keys
            (store_id,event_name,event_id,event_received_at,analytics_event_id)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (store_id,event_name,event_id) DO NOTHING
         RETURNING analytics_event_id",
    )
    .bind(event.store_id)
    .bind(&event.event_name)
    .bind(event.event_id)
    .bind(event.received_at)
    .bind(analytics_event_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    if inserted_key.is_none() {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO integration.analytics_events
            (id,event_id,store_id,channel_id,shopper_id,session_id,utm_source,utm_medium,utm_campaign,utm_term,utm_content,event_name,event_source,properties,occurred_at,received_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
    )
    .bind(analytics_event_id)
    .bind(event.event_id)
    .bind(event.store_id)
    .bind(event.channel_id)
    .bind(event.shopper_id)
    .bind(session_id)
    .bind(utm_source)
    .bind(utm_medium)
    .bind(utm_campaign)
    .bind(utm_term)
    .bind(utm_content)
    .bind(event.event_name)
    .bind(event.event_source)
    .bind(properties)
    .bind(event.occurred_at)
    .bind(event.received_at)
    .execute(&mut **tx)
    .await
    .map_err(db)?;

    Ok(true)
}

/// Splice the ad-platform attribution captured on `commerce.carts` at
/// checkout time (`checkout_attribution_value` in `crate::sales`) into a
/// server-authoritative event's `_meta`. Shared by the InitiateCheckout
/// append at checkout creation and the Purchase append at payment
/// confirmation, so both read the exact same stored snapshot.
pub(crate) fn splice_attribution(properties: &mut Value, attribution: &Value) {
    let Some(object) = properties.as_object_mut() else {
        return;
    };
    let meta = object
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    if let Some(source_url) = attribution.get("source_url") {
        meta.insert("source_url".into(), source_url.clone());
    }
    if let Some(platform_meta) = attribution.get("meta").and_then(Value::as_object) {
        for (key, value) in platform_meta {
            meta.insert(key.clone(), value.clone());
        }
    }
}

/// Server-owned Order contact and shipping identity, hashed into a
/// server-authoritative event's Meta CAPI `user_data`. Shipping fields are
/// only ever known once Stripe has collected them, so they're `None` at
/// checkout creation and populated by the time payment is confirmed.
pub(crate) struct OrderIdentityContext<'a> {
    pub email: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub full_name: Option<&'a str>,
    pub locality: Option<&'a str>,
    pub administrative_area: Option<&'a str>,
    pub postal_code: Option<&'a str>,
    pub country_code: Option<&'a str>,
}

/// Add server-owned order contact identity and the canonical storefront origin
/// without coupling conversion delivery to an arbitrary browser ledger row.
pub(crate) fn merge_order_identity(properties: &mut Value, context: OrderIdentityContext<'_>) {
    let Some(object) = properties.as_object_mut() else {
        return;
    };
    let meta = object
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    if let Some(origin) = context.origin.filter(|value| !value.trim().is_empty()) {
        meta.entry("source_url")
            .or_insert_with(|| Value::String(origin.to_owned()));
    }
    if let Some(email) = context.email.and_then(normalized_email_hash) {
        meta.insert("em".into(), Value::String(email));
    }
    if let Some(phone) = context.phone.and_then(normalized_phone_hash) {
        meta.insert("ph".into(), Value::String(phone));
    }
    let (first_name, last_name) = context
        .full_name
        .map(split_full_name)
        .unwrap_or((None, None));
    for (key, value) in [
        ("fn", first_name.as_deref()),
        ("ln", last_name.as_deref()),
        ("ct", context.locality),
        ("st", context.administrative_area),
        ("zp", context.postal_code),
        ("country", context.country_code),
    ] {
        if let Some(hashed) = value.and_then(normalized_identity_hash) {
            meta.insert(key.into(), Value::String(hashed));
        }
    }
}

/// Chaos only ever collects one shipping name field, so first/last is a
/// lossy split on the first whitespace run — Meta documents this as an
/// acceptable fallback when a store doesn't collect the names separately.
fn split_full_name(full_name: &str) -> (Option<String>, Option<String>) {
    let mut parts = full_name.trim().splitn(2, char::is_whitespace);
    let first = parts.next().filter(|value| !value.is_empty());
    let last = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    (first.map(str::to_owned), last.map(str::to_owned))
}

fn normalized_email_hash(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    (!normalized.is_empty()).then(|| sha256_hex(normalized.as_bytes()))
}

/// Shared normalization for the `fn`/`ln`/`ct`/`st`/`zp`/`country` Meta
/// CAPI fields: trim, lowercase, and drop whitespace. This is a deliberate
/// simplification of Meta's per-field guidance (real postal/state rules are
/// country-specific) — acceptable because Meta's matching tolerates
/// imperfect normalization, and modeling per-country rules isn't worth it
/// for the resulting match-quality gain.
fn normalized_identity_hash(value: &str) -> Option<String> {
    let normalized: String = value
        .trim()
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    (!normalized.is_empty()).then(|| sha256_hex(normalized.as_bytes()))
}

fn normalized_phone_hash(value: &str) -> Option<String> {
    let normalized: String = value.chars().filter(char::is_ascii_digit).collect();
    (!normalized.is_empty()).then(|| sha256_hex(normalized.as_bytes()))
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn normalized_utm_value(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    if value.is_empty() || value.len() > MAX_UTM_VALUE_BYTES || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.to_owned())
}

fn traffic_utm_value(properties: &Value, key: &str) -> Option<String> {
    normalized_utm_value(
        properties
            .get("traffic")
            .and_then(Value::as_object)
            .and_then(|traffic| traffic.get("session"))
            .and_then(Value::as_object)
            .and_then(|session| session.get(key)),
    )
}

impl PostgresAnalyticsEventStore {
    pub(crate) async fn list_events(
        &self,
        actor: StoreActor,
        store: StoreId,
        query: AnalyticsEventQuery,
        limit: u16,
    ) -> Result<AnalyticsEventPage, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let query_limit = i32::from(limit) + 1;
        let rows: Vec<AnalyticsEventRow> = sqlx::query_as(
            "SELECT e.id,e.event_id,e.event_name,e.event_source,e.channel_id,e.shopper_id,e.session_id,
                    e.utm_source,e.utm_medium,e.utm_campaign,e.utm_term,e.utm_content,
                    e.occurred_at,e.received_at,e.properties,
                    COALESCE(delivery_snapshot.deliveries, '[]'::jsonb)
             FROM integration.analytics_events e
             LEFT JOIN LATERAL (
                 SELECT jsonb_agg(jsonb_build_object(
                            'provider', destination.provider,
                            'status', delivery.delivery_status::text,
                            'delivered_at', delivery.delivered_at,
                            'provider_reference', delivery.provider_reference,
                            'last_error', delivery.last_error
                        ) ORDER BY destination.provider) AS deliveries
                 FROM integration.analytics_deliveries AS delivery
                 INNER JOIN integration.analytics_destinations AS destination
                    ON destination.store_id = delivery.store_id
                   AND destination.id = delivery.destination_id
                 WHERE delivery.store_id = e.store_id
                   AND delivery.analytics_event_received_at = e.received_at
                   AND delivery.analytics_event_id = e.id
             ) AS delivery_snapshot ON true
             WHERE e.store_id=$1
               AND (
                   ($3::timestamptz IS NULL AND $4::uuid IS NULL)
                   OR (
                       $3::timestamptz IS NOT NULL
                       AND $4::uuid IS NOT NULL
                       AND (e.received_at, e.id) < ($3, $4)
                   )
               )
               AND ($5::text IS NULL OR e.event_name=$5)
               AND ($6::text IS NULL OR e.event_source=$6)
               AND ($7::text IS NULL OR EXISTS (
                   SELECT 1 FROM integration.analytics_deliveries filter_delivery
                    WHERE filter_delivery.store_id=e.store_id
                      AND filter_delivery.analytics_event_received_at=e.received_at
                      AND filter_delivery.analytics_event_id=e.id
                      AND filter_delivery.delivery_status=$7::integration.delivery_status
               ))
               AND ($8::uuid IS NULL OR e.shopper_id=$8)
               AND ($9::uuid IS NULL OR e.channel_id=$9)
               AND ($10::uuid IS NULL OR e.session_id=$10)
               AND ($11::text IS NULL OR e.utm_source=$11)
               AND ($12::text IS NULL OR e.utm_medium=$12)
               AND ($13::text IS NULL OR e.utm_campaign=$13)
               AND ($14::text IS NULL OR e.utm_term=$14)
               AND ($15::text IS NULL OR e.utm_content=$15)
             ORDER BY e.received_at DESC, e.id DESC
             LIMIT $2",
        )
        .bind(store.as_uuid())
        .bind(query_limit)
        .bind(query.before_received_at)
        .bind(query.before_id)
        .bind(query.event_name)
        .bind(query.source)
        .bind(query.delivery_status)
        .bind(query.shopper_id)
        .bind(query.channel_id)
        .bind(query.session_id)
        .bind(query.utm_source)
        .bind(query.utm_medium)
        .bind(query.utm_campaign)
        .bind(query.utm_term)
        .bind(query.utm_content)
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
                    event_source,
                    channel_id,
                    shopper_id,
                    session_id,
                    utm_source,
                    utm_medium,
                    utm_campaign,
                    utm_term,
                    utm_content,
                    occurred_at,
                    received_at,
                    properties,
                    deliveries,
                )|
                 -> Result<AnalyticsEventRecord, ApplicationError> {
                    let deliveries = deliveries
                        .0
                        .into_iter()
                        .map(|delivery| {
                            Ok(AnalyticsEventDelivery {
                                provider: delivery.provider,
                                status: delivery.status,
                                delivered_at: parse_optional_rfc3339(delivery.delivered_at)?,
                                provider_reference: delivery.provider_reference,
                                last_error: delivery.last_error,
                            })
                        })
                        .collect::<Result<Vec<_>, ApplicationError>>()?;
                    Ok(AnalyticsEventRecord {
                        id,
                        event_id,
                        event_name,
                        event_source,
                        channel_id,
                        shopper_id,
                        session_id,
                        utm_source,
                        utm_medium,
                        utm_campaign,
                        utm_term,
                        utm_content,
                        occurred_at,
                        received_at,
                        properties,
                        deliveries,
                    })
                },
            )
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        Ok(AnalyticsEventPage { events, has_more })
    }
}

fn parse_optional_rfc3339(
    value: Option<String>,
) -> Result<Option<OffsetDateTime>, ApplicationError> {
    match value {
        Some(value) => OffsetDateTime::parse(&value, &Rfc3339)
            .map(Some)
            .map_err(|error| {
                ApplicationError::Unexpected(anyhow::anyhow!(
                    "invalid analytics delivery timestamp {value:?}: {error}"
                ))
            }),
        None => Ok(None),
    }
}

impl PostgresAnalyticsDestinationStore {
    pub(crate) async fn get_destination(
        &self,
        actor: StoreActor,
        store: StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsDestination>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let row: Option<AnalyticsDestinationRow> = sqlx::query_as(
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

    pub(crate) async fn configure_destination(
        &self,
        actor: StoreActor,
        store: StoreId,
        configuration: AnalyticsDestinationConfiguration,
        now: OffsetDateTime,
    ) -> Result<AnalyticsDestination, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
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
                   FROM integration.configure_analytics_destination($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(store.as_uuid())
        .bind(configuration.provider)
        .bind(configuration.external_account_reference)
        .bind(configuration.credential_secret_reference)
        .bind(configuration.configuration)
        .bind(configuration.enabled)
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
        tx.commit().await.map_err(db)?;
        Ok(result)
    }
}

impl PostgresAnalyticsDeliveryStore {
    pub(crate) async fn schedule_deliveries(&self, limit: u16) -> Result<usize, ApplicationError> {
        let scheduled: Option<i64> =
            sqlx::query_scalar("SELECT integration.schedule_analytics_deliveries($1)")
                .bind(i32::from(limit))
                .fetch_one(&self.pool)
                .await
                .map_err(db)?;
        Ok(usize::try_from(scheduled.unwrap_or_default()).unwrap_or(usize::MAX))
    }

    pub(crate) async fn claim_deliveries(
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

    pub(crate) async fn load_delivery(
        &self,
        job: &AnalyticsDeliveryJob,
    ) -> Result<AnalyticsDeliveryCommand, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, job.store_id.as_uuid(), None).await?;
        let row: (Uuid, String, String, String, Value, String, String, OffsetDateTime, Uuid, Value) =
            sqlx::query_as(
                "SELECT e.event_id,destination.provider,destination.external_account_reference,
                        destination.credential_secret_reference,destination.configuration,
                        e.event_name,e.event_source,e.occurred_at,e.shopper_id,e.properties
                   FROM integration.analytics_deliveries delivery
                   JOIN integration.analytics_events e
                     ON e.store_id=delivery.store_id
                    AND e.received_at=delivery.analytics_event_received_at
                    AND e.id=delivery.analytics_event_id
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
        let properties = row.9;
        tx.commit().await.map_err(db)?;
        Ok(AnalyticsDeliveryCommand {
            delivery_id: job.id,
            provider: row.1,
            event_id: row.0,
            external_account_reference: row.2,
            credential_secret_reference: row.3,
            configuration: row.4,
            event_name: row.5,
            event_source: row.6,
            occurred_at: row.7,
            shopper_id: row.8,
            properties,
        })
    }

    pub(crate) async fn finish_delivery(
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
            "SELECT integration.finish_analytics_event_delivery($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(job.id)
        .bind(i32::try_from(job.attempts).unwrap_or(i32::MAX))
        .bind(MAX_INTEGRATION_ATTEMPTS)
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

fn db(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use super::{
        OrderIdentityContext, merge_order_identity, normalized_utm_value, parse_optional_rfc3339,
        sha256_hex, splice_attribution, traffic_utm_value,
    };
    use serde_json::json;

    #[test]
    fn parses_json_delivery_timestamps() {
        let timestamp = parse_optional_rfc3339(Some("2026-08-27T09:15:53.535416+00:00".into()))
            .expect("timestamp should parse")
            .expect("timestamp should be present");
        assert_eq!(timestamp.year(), 2026);
        assert_eq!(timestamp.hour(), 9);
        assert_eq!(timestamp.minute(), 15);
        assert_eq!(
            parse_optional_rfc3339(None).expect("null should parse"),
            None
        );
    }

    #[test]
    fn accepts_flexible_utm_values_without_assigning_semantics() {
        let value = json!({"utm_source": "  partner/A  "});
        assert_eq!(
            normalized_utm_value(value.get("utm_source")),
            Some("partner/A".into())
        );
    }

    #[test]
    fn reads_session_utm_values_from_traffic_history() {
        let value = json!({
            "traffic": {
                "session": {
                    "source": "newsletter",
                    "medium": "email"
                }
            }
        });
        assert_eq!(
            traffic_utm_value(&value, "source"),
            Some("newsletter".into())
        );
        assert_eq!(traffic_utm_value(&value, "medium"), Some("email".into()));
    }

    #[test]
    fn hashes_and_splits_shipping_identity_for_meta_matching() {
        let mut properties = json!({});
        merge_order_identity(
            &mut properties,
            OrderIdentityContext {
                email: None,
                phone: None,
                origin: None,
                full_name: Some("Jane Q. Shopper"),
                locality: Some("San Francisco"),
                administrative_area: Some("CA"),
                postal_code: Some("94103"),
                country_code: Some("US"),
            },
        );

        let meta = &properties["_meta"];
        assert_eq!(
            meta["fn"],
            json!(sha256_hex(b"jane")),
            "full_name splits on the first whitespace run"
        );
        assert_eq!(meta["ln"], json!(sha256_hex(b"q.shopper")));
        assert_eq!(meta["ct"], json!(sha256_hex(b"sanfrancisco")));
        assert_eq!(meta["st"], json!(sha256_hex(b"ca")));
        assert_eq!(meta["zp"], json!(sha256_hex(b"94103")));
        assert_eq!(meta["country"], json!(sha256_hex(b"us")));
    }

    #[test]
    fn splices_cart_attribution_source_url_and_platform_meta() {
        let mut properties = json!({"order_id": "o-1"});
        let attribution = json!({
            "source_url": "https://shop.example/checkout",
            "meta": {"fbc": "fb.1.123.click", "fbp": "fb.1.123.browser"}
        });

        splice_attribution(&mut properties, &attribution);

        assert_eq!(
            properties["_meta"]["source_url"],
            "https://shop.example/checkout"
        );
        assert_eq!(properties["_meta"]["fbc"], "fb.1.123.click");
        assert_eq!(properties["_meta"]["fbp"], "fb.1.123.browser");
    }
}
