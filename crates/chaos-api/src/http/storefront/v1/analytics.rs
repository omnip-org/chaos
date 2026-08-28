use std::net::IpAddr;

use axum::{
    Router,
    extract::State,
    http::{
        HeaderMap,
        header::{COOKIE, USER_AGENT},
    },
    routing::post,
};
use chaos_core::{
    ApplicationError,
    analytics::{BrowserEventCollectionResult, CollectBrowserEventsInput},
    contracts::AnalyticsEventInput,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::http::shared::response::parse_api_time;
use crate::http::{AnalyticsShopper, ApiError, ApiJson, ApiResponse, ApiState};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new().route("/analytics/events", post(collect_events))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectEventsBody {
    events: Vec<AnalyticsEventBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnalyticsEventBody {
    event_id: Uuid,
    event_name: String,
    occurred_at: String,
    properties: serde_json::Value,
}

#[derive(Serialize)]
struct CollectionResultData {
    received: usize,
    stored: usize,
    duplicates: usize,
}

async fn collect_events(
    State(state): State<ApiState>,
    AnalyticsShopper(shopper): AnalyticsShopper,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CollectEventsBody>,
) -> Result<ApiResponse<CollectionResultData>, ApiError> {
    let request_meta = request_meta(&headers);
    let events = body
        .events
        .into_iter()
        .map(|event| {
            Ok::<_, ApiError>(AnalyticsEventInput {
                event_id: event.event_id,
                event_name: event.event_name,
                occurred_at: parse_api_time(&event.occurred_at).map_err(|_| {
                    invalid_value("events.occurred_at", "must be an RFC 3339 timestamp")
                })?,
                properties: merge_request_meta(event.properties, &request_meta),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let result = state
        .analytics_collection
        .collect(CollectBrowserEventsInput {
            actor: shopper.machine,
            shopper_id: shopper.shopper_id.as_uuid(),
            events,
            received_at: state.clock.now(),
        })
        .await
        .inspect_err(|error| {
            if matches!(error, ApplicationError::RateLimited { .. }) {
                tracing::debug!("analytics collection request rate limited");
            }
        })?;
    Ok(ApiResponse::ok(collection_result_data(result)))
}

/// Browser-controlled event properties are useful for the ledger, but the
/// request is the trusted place to enrich network context. Matching cookies
/// belong to the event's capture time, so an event-provided fbc/fbp wins when
/// present; request cookies only fill a missing value. Network-derived UA/IP
/// values are always replaced with the values from the request.
fn request_meta(headers: &HeaderMap) -> Map<String, Value> {
    let mut meta = Map::new();
    if let Some(value) = cookie(headers, "_fbc").filter(|value| valid_meta_browser_id(value)) {
        meta.insert("fbc".into(), Value::String(value));
    }
    if let Some(value) = cookie(headers, "_fbp").filter(|value| valid_meta_browser_id(value)) {
        meta.insert("fbp".into(), Value::String(value));
    }
    if let Some(value) = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty() && value.len() <= 512)
    {
        meta.insert("client_user_agent".into(), Value::String(value.to_owned()));
    }
    if let Some(value) = client_ip(headers) {
        meta.insert("client_ip_address".into(), Value::String(value));
    }
    meta
}

fn merge_request_meta(mut properties: Value, request_meta: &Map<String, Value>) -> Value {
    if request_meta.is_empty() {
        return properties;
    }
    let Some(object) = properties.as_object_mut() else {
        return properties;
    };
    let mut meta = object
        .remove("_meta")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    for (key, value) in request_meta {
        match key.as_str() {
            // A queued event can be flushed after the browser has received a
            // different campaign click. Preserve the matching context that
            // was captured with this event and use the request cookie only as
            // a fallback.
            "fbc" | "fbp" => {
                meta.entry(key.clone()).or_insert_with(|| value.clone());
            }
            // These values describe the request that delivered the event and
            // should not be trusted from a browser-controlled JSON body.
            _ => {
                meta.insert(key.clone(), value.clone());
            }
        }
    }
    object.insert("_meta".into(), Value::Object(meta));
    properties
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|header| {
            header.split(';').find_map(|part| {
                let (key, value) = part.trim().split_once('=')?;
                (key == name && !value.trim().is_empty()).then(|| value.trim().to_owned())
            })
        })
        .filter(|value| value.len() <= 512)
}

fn valid_meta_browser_id(value: &str) -> bool {
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

fn client_ip(headers: &HeaderMap) -> Option<String> {
    ["x-forwarded-for", "x-real-ip"]
        .into_iter()
        .filter_map(|name| headers.get(name).and_then(|value| value.to_str().ok()))
        .flat_map(|value| value.split(','))
        .find_map(|value| value.trim().parse::<IpAddr>().ok())
        .map(|value| value.to_string())
}

fn collection_result_data(result: BrowserEventCollectionResult) -> CollectionResultData {
    CollectionResultData {
        received: result.received,
        stored: result.stored,
        duplicates: result.duplicates,
    }
}

fn invalid_value(field: &'static str, reason: &'static str) -> ApiError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn request_context_reads_matching_cookies_user_agent_and_forwarded_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static(
                "_fbp=fb.1.1234567890123.browser; _fbc=fb.1.1234567890123.click",
            ),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("ChaosBrowser/1.0"));
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.8, 10.0.0.2"),
        );
        let meta = request_meta(&headers);
        assert_eq!(meta["fbp"], "fb.1.1234567890123.browser");
        assert_eq!(meta["fbc"], "fb.1.1234567890123.click");
        assert_eq!(meta["client_user_agent"], "ChaosBrowser/1.0");
        assert_eq!(meta["client_ip_address"], "203.0.113.8");
    }

    #[test]
    fn request_context_ignores_invalid_browser_matching_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_static("_fbc=fb.1.123.click"));

        let meta = request_meta(&headers);

        assert!(meta.get("fbc").is_none());
    }

    #[test]
    fn request_context_preserves_event_matching_and_overrides_network_context() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static(
                "_fbp=fb.1.1234567890123.current; _fbc=fb.1.1234567890123.current-click",
            ),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("ChaosBrowser/2.0"));
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.10"));
        let properties = serde_json::json!({
            "path": "/products",
            "_meta": {
                "fbp": "fb.1.1234567890123.event",
                "fbc": "fb.1.1234567890123.event-click",
                "client_user_agent": "EventBrowser/1.0",
                "client_ip_address": "203.0.113.9",
                "source_url": "https://shop.example/products"
            }
        });
        let merged = merge_request_meta(properties, &request_meta(&headers));
        assert_eq!(merged["_meta"]["fbp"], "fb.1.1234567890123.event");
        assert_eq!(merged["_meta"]["fbc"], "fb.1.1234567890123.event-click");
        assert_eq!(merged["_meta"]["client_user_agent"], "ChaosBrowser/2.0");
        assert_eq!(merged["_meta"]["client_ip_address"], "203.0.113.10");
        assert_eq!(
            merged["_meta"]["source_url"],
            "https://shop.example/products"
        );
    }
}
