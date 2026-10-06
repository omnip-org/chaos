use serde_json::{Map, Value, json};
use url::Url;

const MAX_CHECKOUT_ATTRIBUTION_JSON_BYTES: usize = 3 * 1024;

#[derive(Default)]
pub struct ShopperSessionContext {
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub utm: UtmTags,
}

#[derive(Default)]
pub struct UtmTags {
    pub source: Option<String>,
    pub medium: Option<String>,
    pub campaign: Option<String>,
    pub term: Option<String>,
    pub content: Option<String>,
}

#[derive(Default)]
pub struct CheckoutAttributionInput {
    pub meta_fbc: Option<String>,
    pub meta_fbp: Option<String>,
    pub client_ip_address: Option<String>,
    pub client_user_agent: Option<String>,
    pub source_url: Option<String>,
    pub utm: UtmTags,
}

pub(super) fn checkout_attribution_value(input: CheckoutAttributionInput) -> Option<Value> {
    let mut meta = Map::new();
    for (key, value) in [
        ("fbc", input.meta_fbc),
        ("fbp", input.meta_fbp),
        ("client_ip_address", input.client_ip_address),
        ("client_user_agent", input.client_user_agent),
    ] {
        insert_string(&mut meta, key, value);
    }

    let mut attribution = Map::new();
    if let Some(source_url) = sanitized_source_url(input.source_url) {
        attribution.insert("source_url".into(), Value::String(source_url));
    }
    let utm = utm_map(input.utm);
    if !utm.is_empty() {
        attribution.insert("utm".into(), Value::Object(utm));
    }
    if !meta.is_empty() {
        attribution.insert("meta".into(), Value::Object(meta));
    }

    if serde_json::to_vec(&attribution)
        .is_ok_and(|value| value.len() > MAX_CHECKOUT_ATTRIBUTION_JSON_BYTES)
    {
        attribution.remove("utm");
    }
    (!attribution.is_empty()).then_some(Value::Object(attribution))
}

pub(super) fn shopper_session_attribution(context: ShopperSessionContext) -> Option<Value> {
    let snapshot = shopper_seen_snapshot(context)?;
    Some(json!({ "first_seen": snapshot.clone(), "last_seen": snapshot }))
}

pub(super) fn shopper_seen_snapshot(context: ShopperSessionContext) -> Option<Value> {
    let mut snapshot = Map::new();
    insert_string(&mut snapshot, "user_agent", context.user_agent);
    insert_string(&mut snapshot, "ip", context.ip_address);

    let utm = utm_map(context.utm);
    if !utm.is_empty() {
        snapshot.insert("utm".into(), Value::Object(utm));
    }
    (!snapshot.is_empty()).then_some(Value::Object(snapshot))
}

fn utm_map(utm: UtmTags) -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in [
        ("utm_source", utm.source),
        ("utm_medium", utm.medium),
        ("utm_campaign", utm.campaign),
        ("utm_term", utm.term),
        ("utm_content", utm.content),
    ] {
        insert_string(&mut map, key, value);
    }
    map
}

fn insert_string(map: &mut Map<String, Value>, key: &'static str, value: Option<String>) {
    if let Some(value) = sanitized_string(value) {
        map.insert(key.into(), Value::String(value));
    }
}

fn sanitized_string(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
    })
}

fn sanitized_source_url(value: Option<String>) -> Option<String> {
    let value = sanitized_string(value)?;
    let url = Url::parse(&value).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_keeps_opaque_meta_ids_and_drops_invalid_source_url() {
        let attribution = checkout_attribution_value(CheckoutAttributionInput {
            meta_fbc: Some("future-format-click-id".into()),
            meta_fbp: Some("future-format-browser-id".into()),
            source_url: Some("javascript:alert(1)".into()),
            ..CheckoutAttributionInput::default()
        })
        .expect("meta identifiers produce attribution");

        assert_eq!(attribution["meta"]["fbc"], "future-format-click-id");
        assert_eq!(attribution["meta"]["fbp"], "future-format-browser-id");
        assert!(attribution.get("source_url").is_none());
    }

    #[test]
    fn checkout_drops_utm_before_the_storage_limit() {
        let long = "x".repeat(512);
        let attribution = checkout_attribution_value(CheckoutAttributionInput {
            meta_fbc: Some(long.clone()),
            meta_fbp: Some(long.clone()),
            client_ip_address: Some("2001:db8::1".into()),
            client_user_agent: Some(long.clone()),
            source_url: Some(format!("https://shop.example/{}", "x".repeat(480))),
            utm: UtmTags {
                source: Some(long.clone()),
                medium: Some(long.clone()),
                campaign: Some(long.clone()),
                term: Some(long.clone()),
                content: Some(long),
            },
        })
        .expect("higher-priority attribution remains");

        assert!(attribution.get("utm").is_none());
        assert!(
            serde_json::to_vec(&attribution).unwrap().len() <= MAX_CHECKOUT_ATTRIBUTION_JSON_BYTES
        );
        assert!(attribution["meta"].get("fbc").is_some());
        assert!(attribution.get("source_url").is_some());
    }

    #[test]
    fn shopper_session_records_first_and_last_seen() {
        let context = ShopperSessionContext {
            user_agent: Some("Mozilla/5.0".into()),
            ip_address: Some("203.0.113.7".into()),
            utm: UtmTags {
                source: Some("newsletter".into()),
                medium: Some("email".into()),
                ..UtmTags::default()
            },
        };

        let attribution = shopper_session_attribution(context).expect("some attribution");
        let expected = json!({
            "user_agent": "Mozilla/5.0",
            "ip": "203.0.113.7",
            "utm": {"utm_source": "newsletter", "utm_medium": "email"},
        });

        assert_eq!(attribution["first_seen"], expected);
        assert_eq!(attribution["last_seen"], expected);
    }

    #[test]
    fn empty_attribution_has_no_storage_value() {
        assert!(checkout_attribution_value(CheckoutAttributionInput::default()).is_none());
        assert!(shopper_session_attribution(ShopperSessionContext::default()).is_none());
        assert!(shopper_seen_snapshot(ShopperSessionContext::default()).is_none());
    }
}
