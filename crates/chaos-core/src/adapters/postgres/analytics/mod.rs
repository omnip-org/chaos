use crate::{ApplicationError, contracts::*, store::StoreActor};
use chaos_domain::store::StoreId;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

const MAX_META_BROWSER_ID_BYTES: usize = 2_048;
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

type CheckoutAttributionRow = (
    Value,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
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
    sqlx::query(
        "SELECT pg_advisory_xact_lock(\
            hashtextextended($1::text || ':' || $2::text || ':' || $3::text, 0)\
        )",
    )
    .bind(event.store_id)
    .bind(&event.event_name)
    .bind(event.event_id)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM integration.analytics_events \
         WHERE store_id = $1 AND event_name = $2 AND event_id = $3)",
    )
    .bind(event.store_id)
    .bind(&event.event_name)
    .bind(event.event_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(db)?;
    if duplicate {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO integration.analytics_events
            (id,event_id,store_id,shopper_id,session_id,utm_source,utm_medium,utm_campaign,utm_term,utm_content,event_name,properties,occurred_at,received_at)
         VALUES (uuidv7(),$1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(event.event_id)
    .bind(event.store_id)
    .bind(event.shopper_id)
    .bind(session_id)
    .bind(utm_source)
    .bind(utm_medium)
    .bind(utm_campaign)
    .bind(utm_term)
    .bind(utm_content)
    .bind(event.event_name)
    .bind(properties)
    .bind(event.occurred_at)
    .bind(event.received_at)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok(true)
}

/// Keep only the attribution context that is safe and useful to carry from the
/// exact browser checkout event into a server-authoritative Purchase. In
/// particular, this never queries another browser event: the event associated
/// with the Order is the sole source of fbc/fbp, session, traffic, URL, UA, and
/// IP.
pub(crate) fn attribution_from_properties(properties: &Value) -> Value {
    let Some(object) = properties.as_object() else {
        return Value::Object(Map::new());
    };
    let mut snapshot = Map::new();
    if let Some(Value::Object(meta)) = object.get("_meta") {
        let mut selected = Map::new();
        for key in [
            "source_url",
            "fbc",
            "fbp",
            "client_ip_address",
            "client_user_agent",
        ] {
            if let Some(value) = meta
                .get(key)
                .filter(|value| attribution_value_is_safe(key, value))
            {
                selected.insert(key.into(), value.clone());
            }
        }
        if !selected.is_empty() {
            snapshot.insert("_meta".into(), Value::Object(selected));
        }
    }
    for key in [
        "session_id",
        "traffic",
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
    ] {
        if let Some(value) = object
            .get(key)
            .filter(|value| attribution_value_is_safe(key, value))
        {
            snapshot.insert(key.into(), value.clone());
        }
    }
    let result = Value::Object(snapshot);
    if serde_json::to_vec(&result)
        .map(|value| value.len() <= 32_768)
        .unwrap_or(false)
    {
        result
    } else {
        Value::Object(Map::new())
    }
}

/// Merge an event's captured attribution into a server-authoritative event.
/// Existing server values win, so request-derived network context and order
/// identity cannot be overwritten by a browser payload.
pub(crate) fn merge_attribution(properties: &mut Value, attribution: &Value) {
    let (Some(target), Some(source)) = (properties.as_object_mut(), attribution.as_object()) else {
        return;
    };
    if let Some(Value::Object(source_meta)) = source.get("_meta") {
        let target_meta = target
            .entry("_meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(target_meta) = target_meta.as_object_mut() {
            for (key, value) in source_meta {
                target_meta
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    for key in [
        "session_id",
        "traffic",
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
    ] {
        if let Some(value) = source.get(key) {
            target.entry(key).or_insert_with(|| value.clone());
        }
    }
}

/// Load attribution from the exact browser-side InitiateCheckout event that
/// the SDK records after an Order is created. A later payment webhook can
/// correlate by order_id without using the latest browser event or adding
/// attribution columns to commerce.orders.
pub(crate) async fn load_checkout_attribution(
    tx: &mut Transaction<'_, Postgres>,
    store_id: Uuid,
    order_id: Uuid,
) -> Result<Value, ApplicationError> {
    let row: Option<CheckoutAttributionRow> = sqlx::query_as(
        "SELECT properties,session_id,utm_source,utm_medium,utm_campaign,utm_term,utm_content
            FROM integration.analytics_events
          WHERE store_id = $1
            AND event_name = 'initiate_checkout'
            AND properties->>'_source' = 'browser'
            AND properties->>'order_id' = $2
          ORDER BY occurred_at DESC, received_at DESC, id DESC
          LIMIT 1",
    )
    .bind(store_id)
    .bind(order_id.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    let Some((properties, session_id, utm_source, utm_medium, utm_campaign, utm_term, utm_content)) =
        row
    else {
        return Ok(Value::Object(Map::new()));
    };
    let mut attribution = attribution_from_properties(&properties);
    if let Some(object) = attribution.as_object_mut() {
        if let Some(session_id) = session_id {
            object.insert("session_id".into(), Value::String(session_id.to_string()));
        }
        for (key, value) in [
            ("utm_source", utm_source),
            ("utm_medium", utm_medium),
            ("utm_campaign", utm_campaign),
            ("utm_term", utm_term),
            ("utm_content", utm_content),
        ] {
            if let Some(value) = value {
                object.insert(key.into(), Value::String(value));
            }
        }
    }
    Ok(attribution)
}

/// Add server-owned order contact identity and the canonical storefront origin
/// without coupling conversion delivery to an arbitrary browser ledger row.
pub(crate) fn merge_order_identity(
    properties: &mut Value,
    email: Option<&str>,
    phone: Option<&str>,
    origin: Option<&str>,
) {
    let Some(object) = properties.as_object_mut() else {
        return;
    };
    let meta = object
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    if let Some(origin) = origin.filter(|value| !value.trim().is_empty()) {
        meta.entry("source_url")
            .or_insert_with(|| Value::String(origin.to_owned()));
    }
    if let Some(email) = email.and_then(normalized_email_hash) {
        meta.insert("em".into(), Value::String(email));
    }
    if let Some(phone) = phone.and_then(normalized_phone_hash) {
        meta.insert("ph".into(), Value::String(phone));
    }
}

fn attribution_value_is_safe(key: &str, value: &Value) -> bool {
    match key {
        "fbc" | "fbp" => value.as_str().is_some_and(valid_meta_browser_id),
        "session_id" => value
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_some(),
        "utm_source" | "utm_medium" | "utm_campaign" | "utm_term" | "utm_content" => {
            normalized_utm_value(Some(value)).is_some()
        }
        "source_url" => value
            .as_str()
            .is_some_and(|value| value.len() <= 2_048 && !value.chars().any(char::is_control)),
        "client_ip_address" => value
            .as_str()
            .is_some_and(|value| value.parse::<std::net::IpAddr>().is_ok()),
        "client_user_agent" => value.as_str().is_some_and(|value| {
            !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
        }),
        "traffic" => value.is_object(),
        _ => false,
    }
}

fn valid_meta_browser_id(value: &str) -> bool {
    if value.len() > MAX_META_BROWSER_ID_BYTES {
        return false;
    }
    let mut parts = value.splitn(4, '.');
    let (Some(prefix), Some(version), Some(timestamp), Some(suffix)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    prefix == "fb"
        && !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_digit())
        && timestamp.len() == 13
        && timestamp.bytes().all(|byte| byte.is_ascii_digit())
        && !suffix.is_empty()
        && !suffix.chars().any(char::is_whitespace)
}

fn normalized_email_hash(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
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
    pub(crate) async fn append_events(
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
                JOIN commerce.store_sales_channels c ON c.store_id=s.id
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
            "SELECT e.id,e.event_id,e.event_name,e.shopper_id,e.session_id,
                    e.utm_source,e.utm_medium,e.utm_campaign,e.utm_term,e.utm_content,
                    e.occurred_at,e.received_at,e.properties,
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
               AND ($8::uuid IS NULL OR e.session_id=$8)
               AND ($9::text IS NULL OR e.utm_source=$9)
               AND ($10::text IS NULL OR e.utm_medium=$10)
               AND ($11::text IS NULL OR e.utm_campaign=$11)
               AND ($12::text IS NULL OR e.utm_term=$12)
               AND ($13::text IS NULL OR e.utm_content=$13)
             GROUP BY e.id,e.event_id,e.event_name,e.shopper_id,e.session_id,
                      e.utm_source,e.utm_medium,e.utm_campaign,e.utm_term,e.utm_content,
                      e.occurred_at,e.received_at,e.properties
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
        let properties = row.8;
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
        attribution_from_properties, merge_attribution, normalized_utm_value,
        parse_optional_rfc3339, traffic_utm_value,
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
    fn extracts_only_event_attribution_for_later_server_conversions() {
        let session_id = "11111111-1111-4111-8111-111111111111";
        let properties = json!({
            "_meta": {
                "source_url": "https://shop.example/products",
                "fbc": "fb.1.1234567890123.click",
                "fbp": "fb.1.1234567890123.browser",
                "client_ip_address": "203.0.113.8",
                "client_user_agent": "ChaosBrowser/1.0",
                "em": "raw-email-must-not-be-copied"
            },
            "session_id": session_id,
            "traffic": {"session": {"source": "newsletter"}},
            "utm_source": "newsletter",
            "utm_medium": "email",
            "em": "raw-email-must-not-be-copied",
            "product_id": "product-is-not-attribution"
        });

        let snapshot = attribution_from_properties(&properties);

        assert_eq!(snapshot["_meta"]["fbc"], "fb.1.1234567890123.click");
        assert_eq!(snapshot["_meta"]["fbp"], "fb.1.1234567890123.browser");
        assert_eq!(snapshot["session_id"], session_id);
        assert_eq!(snapshot["utm_source"], "newsletter");
        assert!(snapshot["_meta"].get("em").is_none());
        assert!(snapshot.get("em").is_none());
        assert!(snapshot.get("product_id").is_none());
    }

    #[test]
    fn merges_captured_attribution_without_replacing_server_values() {
        let mut properties = json!({
            "_meta": {
                "source_url": "https://canonical.example/checkout"
            },
            "session_id": "22222222-2222-4222-8222-222222222222"
        });
        let attribution = json!({
            "_meta": {
                "source_url": "https://browser.example/product",
                "fbc": "fb.1.1234567890123.click"
            },
            "session_id": "33333333-3333-4333-8333-333333333333",
            "utm_campaign": "summer"
        });

        merge_attribution(&mut properties, &attribution);

        assert_eq!(
            properties["_meta"]["source_url"],
            "https://canonical.example/checkout"
        );
        assert_eq!(properties["_meta"]["fbc"], "fb.1.1234567890123.click");
        assert_eq!(
            properties["session_id"],
            "22222222-2222-4222-8222-222222222222"
        );
        assert_eq!(properties["utm_campaign"], "summer");
    }
}
