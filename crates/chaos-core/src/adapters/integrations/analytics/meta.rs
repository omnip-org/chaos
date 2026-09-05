//! Meta Conversions API delivery adapter.

use std::{sync::Arc, time::Duration};

use crate::contracts::{
    AnalyticsDeliveryCommand, AnalyticsDeliveryError, AnalyticsDeliveryReceipt,
    AnalyticsEventDestination,
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::adapters::security::provider_secrets::DynamicSecretResolver;

pub struct MetaConversionsDestination {
    client: Client,
    api_base_url: Url,
    secrets: Arc<DynamicSecretResolver>,
}

impl MetaConversionsDestination {
    pub fn new(
        api_base_url: Url,
        timeout: Duration,
        secrets: Arc<DynamicSecretResolver>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            api_base_url.scheme() == "https" || api_base_url.host_str() == Some("127.0.0.1"),
            "Meta API URL must use HTTPS or loopback HTTP"
        );
        let mut api_base_url = api_base_url;
        if !api_base_url.path().ends_with('/') {
            api_base_url.set_path(&format!("{}/", api_base_url.path()));
        }
        Ok(Self {
            client: Client::builder().timeout(timeout).build()?,
            api_base_url,
            secrets,
        })
    }
}

#[async_trait]
impl AnalyticsEventDestination for MetaConversionsDestination {
    fn provider(&self) -> &'static str {
        "meta"
    }

    async fn send(
        &self,
        command: &AnalyticsDeliveryCommand,
    ) -> Result<AnalyticsDeliveryReceipt, AnalyticsDeliveryError> {
        // Keep the first-party ledger broader than Meta's optimization events.
        // Page views, engagement duration, and other internal behavior events
        // are useful in the first-party ledger, but are not sent through Meta
        // CAPI for now. Returning a successful filtered receipt makes the
        // delivery durable without retrying an intentionally excluded event.
        if !is_meta_event(command) {
            return Ok(AnalyticsDeliveryReceipt {
                provider_reference: Some("filtered".into()),
            });
        }
        let token = self
            .secrets
            .resolve_analytics(&command.credential_secret_reference)
            .await?;
        let endpoint = self
            .api_base_url
            .join(&format!("{}/events", command.external_account_reference))
            .map_err(|_| invalid_command())?;
        let payload = MetaRequest {
            data: [MetaEvent {
                event_name: meta_event_name(&command.event_name),
                event_time: command.occurred_at.unix_timestamp(),
                event_id: command.event_id.to_string(),
                action_source: "website",
                event_source_url: source_url(&command.properties),
                user_data: meta_user_data(command),
                custom_data: custom_data(command),
            }],
            test_event_code: command
                .configuration
                .get("test_event_code")
                .and_then(Value::as_str),
        };
        let response = self
            .client
            .post(endpoint)
            .query(&[("access_token", token.expose_secret())])
            .json(&payload)
            .send()
            .await
            .map_err(|error| AnalyticsDeliveryError {
                retryable: true,
                message: format!("Meta request failed: {error}"),
            })?;
        let status = response.status();
        let response_body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(AnalyticsDeliveryError {
                retryable: status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error(),
                message: format!(
                    "Meta returned HTTP {status}: {}",
                    truncate_error_body(&response_body)
                ),
            });
        }
        let receipt: MetaResponse =
            serde_json::from_str(&response_body).map_err(|_| AnalyticsDeliveryError {
                retryable: true,
                message: format!(
                    "Meta returned an invalid response: {}",
                    truncate_error_body(&response_body)
                ),
            })?;
        if receipt.events_received != 1 {
            return Err(AnalyticsDeliveryError {
                retryable: true,
                message: "Meta did not acknowledge the event".into(),
            });
        }
        Ok(AnalyticsDeliveryReceipt {
            provider_reference: receipt.fbtrace_id,
        })
    }
}

#[derive(Serialize)]
struct MetaRequest<'a> {
    data: [MetaEvent<'a>; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    test_event_code: Option<&'a str>,
}

#[derive(Serialize)]
struct MetaEvent<'a> {
    event_name: &'a str,
    event_time: i64,
    event_id: String,
    action_source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_source_url: Option<&'a str>,
    user_data: MetaUserData,
    custom_data: Value,
}

#[derive(Serialize)]
struct MetaUserData {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    external_id: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    em: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ph: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fbc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fbp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_ip_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_user_agent: Option<String>,
    // `fn`/`ln` are Rust keywords, hence the rename.
    #[serde(rename = "fn", skip_serializing_if = "Vec::is_empty")]
    first_name: Vec<String>,
    #[serde(rename = "ln", skip_serializing_if = "Vec::is_empty")]
    last_name: Vec<String>,
    #[serde(rename = "ct", skip_serializing_if = "Vec::is_empty")]
    city: Vec<String>,
    #[serde(rename = "st", skip_serializing_if = "Vec::is_empty")]
    state: Vec<String>,
    #[serde(rename = "zp", skip_serializing_if = "Vec::is_empty")]
    zip: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    country: Vec<String>,
}

#[derive(Deserialize)]
struct MetaResponse {
    events_received: u32,
    fbtrace_id: Option<String>,
}

/// `purchase` and `initiate_checkout` are the only events Chaos sends
/// through Meta CAPI: they're the only ones with a server-confirmed source
/// (a locked Cart handed off to Stripe, and a paid Order). Every other Meta
/// event (ViewContent, AddToCart, Search) is fired client-side only, straight
/// from the storefront's Pixel install — Chaos never sees it.
fn is_meta_event(command: &AnalyticsDeliveryCommand) -> bool {
    command.event_source == "server"
        && matches!(
            command.event_name.as_str(),
            "purchase" | "initiate_checkout"
        )
}

fn meta_event_name(name: &str) -> &str {
    match name {
        "purchase" => "Purchase",
        "initiate_checkout" => "InitiateCheckout",
        _ => name,
    }
}

fn meta_user_data(command: &AnalyticsDeliveryCommand) -> MetaUserData {
    let properties = &command.properties;
    MetaUserData {
        // Hash the stable Chaos shopper identifier in its canonical textual
        // form so it remains stable across retries and destinations.
        external_id: vec![sha256_hex(command.shopper_id.to_string().as_bytes())],
        em: hashed_context_value(properties, "em"),
        ph: hashed_context_value(properties, "ph"),
        fbc: context_value(properties, "fbc")
            .filter(|value| valid_meta_browser_id(value))
            .map(str::to_owned),
        fbp: context_value(properties, "fbp")
            .filter(|value| valid_meta_browser_id(value))
            .map(str::to_owned),
        client_ip_address: context_value(properties, "client_ip_address").map(str::to_owned),
        client_user_agent: context_value(properties, "client_user_agent").map(str::to_owned),
        first_name: hashed_context_value(properties, "fn"),
        last_name: hashed_context_value(properties, "ln"),
        city: hashed_context_value(properties, "ct"),
        state: hashed_context_value(properties, "st"),
        zip: hashed_context_value(properties, "zp"),
        country: hashed_context_value(properties, "country"),
    }
}

fn custom_data(command: &AnalyticsDeliveryCommand) -> Value {
    let Some(mut object) = command.properties.as_object().cloned() else {
        return json!({});
    };

    // These fields are Chaos transport/context data or browser-only display
    // fields. Standard Meta fields are rebuilt below from the canonical Chaos
    // representation instead of forwarding implementation details.
    for key in [
        "_source",
        "_meta",
        "session_id",
        "traffic",
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
        "source_url",
        "path",
        "title",
        "referrer_domain",
        "active_milliseconds",
        "page_view_event_id",
        "product_id",
        "product_variant_id",
        "quantity",
        "query",
        "result_count",
        "items",
        "value_minor",
    ] {
        object.remove(key);
    }

    if let Some(value_minor) = command
        .properties
        .get("value_minor")
        .and_then(Value::as_i64)
    {
        let exponent = command
            .properties
            .get("currency")
            .and_then(Value::as_str)
            .map(currency_exponent)
            .unwrap_or(2);
        object.insert(
            "value".into(),
            json!(value_minor as f64 / 10_f64.powi(exponent)),
        );
    }
    if let Some(currency) = command.properties.get("currency").and_then(Value::as_str) {
        object.insert("currency".into(), json!(currency.to_ascii_uppercase()));
    }
    if let Some(query) = command.properties.get("query").and_then(Value::as_str) {
        object.insert("search_string".into(), json!(query));
    }

    let (contents, content_ids, num_items) = meta_contents(
        command.properties.get("items"),
        command.properties.get("product_variant_id"),
        command.properties.get("product_id"),
        command.properties.get("quantity"),
        command.properties.get("currency").and_then(Value::as_str),
    );
    let has_contents = contents.is_some();
    if let Some(contents) = contents {
        object.insert("contents".into(), contents);
        object.insert("content_ids".into(), json!(content_ids));
        object.insert("content_type".into(), json!("product"));
        // Meta documents num_items as an InitiateCheckout-specific field;
        // Purchase already carries per-line quantity in `contents`.
        if command.event_name == "initiate_checkout" {
            object.insert("num_items".into(), json!(num_items));
        }
    }
    if !has_contents
        && let Some(ids) = content_ids_from_single_product(
            command.properties.get("product_variant_id"),
            command.properties.get("product_id"),
        )
    {
        object.insert("content_ids".into(), json!(ids));
        object.insert("content_type".into(), json!("product"));
    }
    Value::Object(object)
}

fn source_url(properties: &Value) -> Option<&str> {
    context_value(properties, "source_url").filter(|value| {
        Url::parse(value)
            .map(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
            .unwrap_or(false)
    })
}

fn context_value<'a>(properties: &'a Value, key: &str) -> Option<&'a str> {
    properties
        .get("_meta")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            properties
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
        })
}

/// `fn`/`ln`/`ct`/`st`/`zp`/`country` are only ever trusted from `_meta` as
/// already-hashed values (`OrderIdentityContext` hashes them before they're
/// stored) — same guard `em`/`ph` already use.
fn hashed_context_value(properties: &Value, key: &str) -> Vec<String> {
    context_value(properties, key)
        .filter(|value| is_sha256(value))
        .map(str::to_owned)
        .into_iter()
        .collect()
}

fn meta_contents(
    items: Option<&Value>,
    product_variant_id: Option<&Value>,
    product_id: Option<&Value>,
    quantity: Option<&Value>,
    currency: Option<&str>,
) -> (Option<Value>, Vec<String>, i64) {
    let mut contents = Vec::new();
    if let Some(items) = items.and_then(Value::as_array) {
        for item in items {
            let Some(item) = item.as_object() else {
                continue;
            };
            let Some(id) = item
                .get("product_variant_id")
                .or_else(|| item.get("product_id"))
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
            else {
                continue;
            };
            let item_quantity = item
                .get("quantity")
                .and_then(Value::as_i64)
                .filter(|quantity| *quantity > 0)
                .unwrap_or(1);
            let item_price = item
                .get("price_minor")
                .and_then(Value::as_i64)
                .and_then(|minor| currency.map(|currency| minor_to_major(minor, currency)));
            let mut content = Map::new();
            content.insert("id".into(), json!(id));
            content.insert("quantity".into(), json!(item_quantity));
            if let Some(item_price) = item_price {
                content.insert("item_price".into(), json!(item_price));
            }
            contents.push(Value::Object(content));
        }
    }
    if contents.is_empty()
        && let Some(id) = product_variant_id
            .or(product_id)
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
    {
        let item_quantity = quantity
            .and_then(Value::as_i64)
            .filter(|quantity| *quantity > 0)
            .unwrap_or(1);
        let mut content = Map::new();
        content.insert("id".into(), json!(id));
        content.insert("quantity".into(), json!(item_quantity));
        contents.push(Value::Object(content));
    }
    let content_ids = contents
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect::<Vec<_>>();
    let num_items = contents
        .iter()
        .filter_map(|item| item.get("quantity").and_then(Value::as_i64))
        .sum();
    if contents.is_empty() {
        (None, content_ids, num_items)
    } else {
        (Some(Value::Array(contents)), content_ids, num_items)
    }
}

fn content_ids_from_single_product(
    product_variant_id: Option<&Value>,
    product_id: Option<&Value>,
) -> Option<Vec<String>> {
    product_variant_id
        .or(product_id)
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(|id| vec![id.to_owned()])
}

fn minor_to_major(value_minor: i64, currency: &str) -> f64 {
    value_minor as f64 / 10_f64.powi(currency_exponent(currency))
}

fn currency_exponent(currency: &str) -> i32 {
    match currency.to_ascii_uppercase().as_str() {
        "BIF" | "CLP" | "DJF" | "GNF" | "JPY" | "KMF" | "KRW" | "MGA" | "PYG" | "RWF" | "UGX"
        | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "JOD" | "KWD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

fn sha256_hex(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_meta_browser_id(value: &str) -> bool {
    if value.len() > 2_048 {
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

fn invalid_command() -> AnalyticsDeliveryError {
    AnalyticsDeliveryError {
        retryable: false,
        message: "Meta delivery command is invalid".into(),
    }
}

fn truncate_error_body(body: &str) -> String {
    const MAX_ERROR_BYTES: usize = 1024;
    let body = body.trim();
    if body.len() <= MAX_ERROR_BYTES {
        return body.to_owned();
    }
    let mut end = MAX_ERROR_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &body[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn command(value_minor: i64, currency: &str) -> AnalyticsDeliveryCommand {
        AnalyticsDeliveryCommand {
            provider: "meta".into(),
            event_id: Uuid::now_v7(),
            external_account_reference: "12345".into(),
            credential_secret_reference: "env://CHAOS_ANALYTICS_SECRET_TEST".into(),
            configuration: json!({}),
            event_name: "purchase".into(),
            event_source: "server".into(),
            occurred_at: OffsetDateTime::UNIX_EPOCH,
            shopper_id: Uuid::now_v7(),
            properties: json!({"value_minor": value_minor, "currency": currency}),
        }
    }

    #[test]
    fn converts_minor_units_for_meta_without_changing_currency() {
        let usd = custom_data(&command(1_299, "USD"));
        assert_eq!(usd["value"], json!(12.99));
        assert_eq!(usd["currency"], json!("USD"));

        let jpy = custom_data(&command(1_299, "JPY"));
        assert_eq!(jpy["value"], json!(1_299.0));
        assert_eq!(jpy["currency"], json!("JPY"));

        let mga = custom_data(&command(1_299, "mga"));
        assert_eq!(mga["value"], json!(1_299.0));
        assert_eq!(mga["currency"], json!("MGA"));
    }

    #[test]
    fn does_not_forward_traffic_provenance_as_meta_custom_data() {
        let mut input = command(1_299, "USD");
        input.properties = json!({
            "_source":"browser",
            "session_id":"session-1",
            "traffic":{"session":{"fbclid":"private"}},
            "utm_source":"newsletter",
            "path":"/"
        });
        let data = custom_data(&input);
        assert!(data.get("_source").is_none());
        assert!(data.get("session_id").is_none());
        assert!(data.get("traffic").is_none());
        assert!(data.get("utm_source").is_none());
        assert!(data.get("path").is_none());
    }

    #[test]
    fn maps_items_and_search_to_meta_standard_fields() {
        let mut input = command(1_299, "USD");
        // num_items is InitiateCheckout-specific (see the Purchase contract
        // test below, which asserts it's absent there); use a realistic
        // event name here so this test's num_items assertion means something.
        input.event_name = "initiate_checkout".into();
        input.properties = json!({
            "query": "shoes",
            "items": [{"product_id": "product-1", "product_variant_id": "variant-1", "quantity": 2, "price_minor": 650}],
            "currency": "USD",
            "value_minor": 1_300,
            "_meta": {"source_url": "https://shop.example/search?q=shoes"}
        });
        let data = custom_data(&input);
        assert_eq!(data["search_string"], json!("shoes"));
        assert_eq!(data["content_ids"], json!(["variant-1"]));
        assert_eq!(data["contents"][0]["item_price"], json!(6.5));
        assert_eq!(data["num_items"], json!(2));
        assert!(data.get("_meta").is_none());
        assert_eq!(
            source_url(&input.properties),
            Some("https://shop.example/search?q=shoes")
        );
    }

    #[test]
    fn maps_view_content_variant_to_meta_content_fields() {
        let mut input = command(1_299, "USD");
        input.event_name = "view_content".into();
        input.properties = json!({
            "product_id": "product-1",
            "product_variant_id": "variant-1",
        });

        let data = custom_data(&input);

        assert_eq!(data["content_ids"], json!(["variant-1"]));
        assert_eq!(data["content_type"], json!("product"));
        assert_eq!(
            data["contents"],
            json!([{ "id": "variant-1", "quantity": 1 }])
        );
        assert!(data.get("product_id").is_none());
        assert!(data.get("product_variant_id").is_none());
    }

    #[test]
    fn only_server_confirmed_purchase_and_initiate_checkout_are_sent_to_meta() {
        let mut browser = command(1_299, "USD");
        browser.event_source = "browser".into();
        for event_name in [
            "page_view",
            "view_content",
            "search",
            "add_to_cart",
            "initiate_checkout",
            "purchase",
        ] {
            browser.event_name = event_name.into();
            assert!(!is_meta_event(&browser), "browser {event_name}");
        }

        let mut server = browser;
        server.event_source = "server".into();
        for event_name in ["purchase", "initiate_checkout"] {
            server.event_name = event_name.into();
            assert!(is_meta_event(&server), "server {event_name}");
        }
        for event_name in ["page_view", "view_content", "search", "add_to_cart"] {
            server.event_name = event_name.into();
            assert!(!is_meta_event(&server), "server {event_name}");
        }
    }

    #[test]
    fn serializes_the_authoritative_purchase_payload_contract() {
        let mut input = command(1_299, "USD");
        input.properties = json!({
            "_source": "server",
            "_meta": {
                "source_url": "https://shop.example/checkout",
                "fbc": "fb.1.1234567890123.click",
                "fbp": "fb.1.1234567890123.browser"
            },
            "value_minor": 1_299,
            "currency": "USD",
            "items": [{"product_id": "product-1", "product_variant_id": "variant-1", "quantity": 2, "price_minor": 649}]
        });
        let payload = serde_json::to_value(MetaRequest {
            data: [MetaEvent {
                event_name: meta_event_name(&input.event_name),
                event_time: input.occurred_at.unix_timestamp(),
                event_id: input.event_id.to_string(),
                action_source: "website",
                event_source_url: source_url(&input.properties),
                user_data: meta_user_data(&input),
                custom_data: custom_data(&input),
            }],
            test_event_code: None,
        })
        .expect("Meta payload should serialize");

        assert_eq!(payload["data"][0]["event_name"], json!("Purchase"));
        assert_eq!(payload["data"][0]["event_time"], json!(0));
        assert_eq!(
            payload["data"][0]["event_id"],
            json!(input.event_id.to_string())
        );
        assert_eq!(payload["data"][0]["action_source"], json!("website"));
        assert_eq!(
            payload["data"][0]["event_source_url"],
            json!("https://shop.example/checkout")
        );
        assert_eq!(
            payload["data"][0]["user_data"]["fbc"],
            json!("fb.1.1234567890123.click")
        );
        assert_eq!(
            payload["data"][0]["user_data"]["fbp"],
            json!("fb.1.1234567890123.browser")
        );
        assert_eq!(payload["data"][0]["custom_data"]["value"], json!(12.99));
        assert_eq!(payload["data"][0]["custom_data"]["currency"], json!("USD"));
        assert_eq!(
            payload["data"][0]["custom_data"]["contents"],
            json!([{"id": "variant-1", "quantity": 2, "item_price": 6.49}])
        );
        assert_eq!(
            payload["data"][0]["custom_data"]["content_ids"],
            json!(["variant-1"])
        );
        assert_eq!(
            payload["data"][0]["custom_data"]["content_type"],
            json!("product")
        );
        // num_items is InitiateCheckout-specific; Purchase carries per-line
        // quantity in `contents` instead.
        assert!(payload["data"][0]["custom_data"].get("num_items").is_none());
        assert!(payload["data"][0]["custom_data"].get("_source").is_none());
        assert!(payload["data"][0]["custom_data"].get("_meta").is_none());
    }

    #[test]
    fn includes_hashed_identity_and_browser_matching_context() {
        let mut input = command(1_299, "USD");
        input.properties = json!({
            "_meta": {
                "em": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "ph": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "fbc": "fb.1.1234567890123.click",
                "fbp": "fb.1.1234567890123.browser",
                "client_ip_address": "203.0.113.10",
                "client_user_agent": "test-agent",
                "fn": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "ln": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                "ct": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                "st": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "zp": "1111111111111111111111111111111111111111111111111111111111111111",
                "country": "2222222222222222222222222222222222222222222222222222222222222222"
            }
        });
        let user_data = meta_user_data(&input);
        assert_eq!(user_data.em.len(), 1);
        assert_eq!(user_data.ph.len(), 1);
        assert_eq!(user_data.fbc.as_deref(), Some("fb.1.1234567890123.click"));
        assert_eq!(user_data.fbp.as_deref(), Some("fb.1.1234567890123.browser"));
        assert_eq!(user_data.client_ip_address.as_deref(), Some("203.0.113.10"));
        assert_eq!(user_data.client_user_agent.as_deref(), Some("test-agent"));
        assert_eq!(user_data.first_name.len(), 1);
        assert_eq!(user_data.last_name.len(), 1);
        assert_eq!(user_data.city.len(), 1);
        assert_eq!(user_data.state.len(), 1);
        assert_eq!(user_data.zip.len(), 1);
        assert_eq!(user_data.country.len(), 1);
    }

    #[test]
    fn does_not_forward_invalid_browser_matching_ids() {
        let mut input = command(1_299, "USD");
        input.properties = json!({
            "_meta": {
                "fbc": "fb.1.123.click",
                "fbp": "fb.1.123.browser"
            }
        });

        let user_data = meta_user_data(&input);

        assert!(user_data.fbc.is_none());
        assert!(user_data.fbp.is_none());
    }
}
