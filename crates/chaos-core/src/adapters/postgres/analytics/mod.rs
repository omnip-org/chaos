use crate::{ApplicationError, contracts::*, store::StoreActor};
use chaos_domain::store::StoreId;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

/// Meta's destination config (`get_destination`/`configure_destination`,
/// `integration.provider_accounts` with `capability = 'analytics'`) and the
/// `analytics_capi_queue` consumer's credential lookup
/// (`resolve_meta_account`) share this store — both just read/write the
/// same provider account row, no separate analytics-specific table.
pub struct PostgresCapiEventStore {
    pool: PgPool,
}

impl PostgresCapiEventStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

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

/// Publish a topic-routed commerce event (`integration.publish_topic_event`)
/// in the same transaction that produced it, so a rolled-back transaction
/// never delivers a message a consumer would act on. See
/// `migrations/0004_integration.sql` for the queue bindings this reaches.
pub(crate) async fn publish_topic_event(
    tx: &mut Transaction<'_, Postgres>,
    routing_key: &str,
    payload: Value,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT integration.publish_topic_event($1, $2)")
        .bind(routing_key)
        .bind(payload)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}

/// Shared payload shape for `order.payment.initiated`/`order.payment.completed`:
/// enough for the notification-email consumer (`order_id`) and the CAPI
/// consumer (`event_id`/`event_name`/`occurred_at`/`shopper_id`/`properties` —
/// everything `AnalyticsDeliveryCommand` needs to build a Meta CAPI event).
/// `event_id` equals `order_id` for these two routing keys — the browser
/// Pixel copy reuses it so Meta deduplicates the pair — and `event_source`
/// is always `"server"`.
pub(crate) fn payment_event_payload(
    store_id: Uuid,
    order_id: Uuid,
    shopper_id: Uuid,
    event_name: &'static str,
    occurred_at: OffsetDateTime,
    properties: Value,
) -> Value {
    serde_json::json!({
        "store_id": store_id,
        "order_id": order_id,
        "event_id": order_id,
        "event_name": event_name,
        "occurred_at": occurred_at.format(&Rfc3339).unwrap_or_default(),
        "shopper_id": shopper_id,
        "properties": properties,
    })
}

/// Payload for `cart.item.added` (bound to `analytics_capi_queue` in
/// `migrations/0004_integration.sql`). There is no Order yet, so the Meta
/// CAPI `event_id` is minted by the caller inside the cart transaction and
/// carried explicitly; a queue retry replays the same id, and the browser
/// Pixel copy reuses it for Meta's deduplication. `event_source` is always
/// `"server"` for this key.
pub(crate) fn cart_event_payload(
    store_id: Uuid,
    event_id: Uuid,
    shopper_id: Uuid,
    event_name: &'static str,
    occurred_at: OffsetDateTime,
    properties: Value,
) -> Value {
    serde_json::json!({
        "store_id": store_id,
        "event_id": event_id,
        "event_name": event_name,
        "occurred_at": occurred_at.format(&Rfc3339).unwrap_or_default(),
        "shopper_id": shopper_id,
        "properties": properties,
    })
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
    if let Some(utm) = attribution.get("utm").and_then(Value::as_object) {
        for (key, value) in utm {
            meta.insert(key.clone(), value.clone());
        }
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

/// `id, provider, credentials_configured, configuration, enabled, created_at, updated_at`.
type ProviderAccountRow = (
    Uuid,
    String,
    bool,
    Value,
    bool,
    OffsetDateTime,
    OffsetDateTime,
);

/// The Meta provider account's credentials, as `AnalyticsDeliveryCommand`
/// needs them — never exposed outside this crate (unlike `AnalyticsDestination`,
/// which deliberately omits the raw secret reference for MCP responses).
pub(crate) struct MetaAccountCredentials {
    pub(crate) provider: String,
    pub(crate) external_account_reference: String,
    pub(crate) credential_secret_reference: String,
    pub(crate) configuration: Value,
}

impl PostgresCapiEventStore {
    pub(crate) async fn get_destination(
        &self,
        actor: StoreActor,
        store: StoreId,
        provider: &str,
    ) -> Result<Option<AnalyticsDestination>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store.as_uuid(), Some(actor.user_id().as_uuid())).await?;
        let row: Option<ProviderAccountRow> = sqlx::query_as(
            "SELECT id, provider, credential_secret_reference IS NOT NULL, configuration, enabled, created_at, updated_at \
               FROM integration.provider_accounts \
              WHERE store_id=$1 AND capability='analytics' AND provider=$2",
        )
        .bind(store.as_uuid())
        .bind(provider)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(row.map(|row| provider_account_to_destination(store, row)))
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
        let mut merged_configuration = configuration.configuration;
        if let Some(object) = merged_configuration.as_object_mut() {
            object.insert(
                "dataset_id".into(),
                Value::String(configuration.external_account_reference),
            );
        }
        let row: ProviderAccountRow = sqlx::query_as(
            "INSERT INTO integration.provider_accounts \
                (id, store_id, capability, provider, credential_secret_reference, configuration, enabled, created_at, updated_at) \
             VALUES (uuidv7(), $1, 'analytics', $2, $3, $4, $5, $6, $6) \
             ON CONFLICT (store_id, capability, provider) DO UPDATE SET \
                credential_secret_reference = EXCLUDED.credential_secret_reference, \
                configuration = EXCLUDED.configuration, \
                enabled = EXCLUDED.enabled, \
                updated_at = EXCLUDED.updated_at \
             RETURNING id, provider, credential_secret_reference IS NOT NULL, configuration, enabled, created_at, updated_at",
        )
        .bind(store.as_uuid())
        .bind(configuration.provider)
        .bind(configuration.credential_secret_reference)
        .bind(merged_configuration)
        .bind(configuration.enabled)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(provider_account_to_destination(store, row))
    }

    /// Look up Meta's credentials for the `analytics_capi_queue` consumer.
    /// `None` means the Store has no enabled `meta` destination configured
    /// — not a failure, CAPI delivery is best-effort enrichment, same as
    /// attribution capture itself.
    pub(crate) async fn resolve_meta_account(
        &self,
        store_id: Uuid,
    ) -> Result<Option<MetaAccountCredentials>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        context(&mut tx, store_id, None).await?;
        let row: Option<(String, Value)> = sqlx::query_as(
            "SELECT credential_secret_reference, configuration \
               FROM integration.provider_accounts \
              WHERE store_id=$1 AND capability='analytics' AND provider='meta' \
                AND enabled AND credential_secret_reference IS NOT NULL",
        )
        .bind(store_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(row.map(|(credential_secret_reference, configuration)| {
            let external_account_reference = configuration
                .get("dataset_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            MetaAccountCredentials {
                provider: "meta".into(),
                external_account_reference,
                credential_secret_reference,
                configuration,
            }
        }))
    }
}

fn provider_account_to_destination(
    store: StoreId,
    row: ProviderAccountRow,
) -> AnalyticsDestination {
    let (id, provider, credentials_configured, configuration, enabled, created_at, updated_at) =
        row;
    let external_account_reference = configuration
        .get("dataset_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    AnalyticsDestination {
        id,
        store_id: store,
        provider,
        external_account_reference,
        enabled,
        credentials_configured,
        configuration,
        created_at,
        updated_at,
    }
}

fn db(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use super::{
        OrderIdentityContext, cart_event_payload, merge_order_identity, payment_event_payload,
        sha256_hex, splice_attribution,
    };
    use serde_json::json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    #[test]
    fn payment_event_payload_carries_event_id_equal_to_order_id() {
        let order_id = Uuid::now_v7();
        let payload = payment_event_payload(
            Uuid::now_v7(),
            order_id,
            Uuid::now_v7(),
            "purchase",
            OffsetDateTime::UNIX_EPOCH,
            json!({}),
        );
        assert_eq!(payload["order_id"], json!(order_id.to_string()));
        assert_eq!(payload["event_id"], json!(order_id.to_string()));
    }

    #[test]
    fn cart_event_payload_carries_an_explicit_event_id_and_no_order() {
        let event_id = Uuid::now_v7();
        let payload = cart_event_payload(
            Uuid::now_v7(),
            event_id,
            Uuid::now_v7(),
            "add_to_cart",
            OffsetDateTime::UNIX_EPOCH,
            json!({ "value_minor": 500 }),
        );
        assert_eq!(payload["event_id"], json!(event_id.to_string()));
        assert_eq!(payload["event_name"], json!("add_to_cart"));
        assert_eq!(payload["occurred_at"], json!("1970-01-01T00:00:00Z"));
        assert!(payload.get("order_id").is_none());
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

    #[test]
    fn splices_cart_attribution_utm_tags_into_meta() {
        let mut properties = json!({"order_id": "o-1"});
        let attribution = json!({
            "utm": {"utm_source": "newsletter", "utm_medium": "email"}
        });

        splice_attribution(&mut properties, &attribution);

        assert_eq!(properties["_meta"]["utm_source"], "newsletter");
        assert_eq!(properties["_meta"]["utm_medium"], "email");
    }
}
