use axum::{Router, extract::State, http::HeaderMap, routing::post};
use chaos_core::{
    ApplicationError,
    analytics::{BrowserEventCollectionResult, CollectBrowserEventsInput},
};
use serde::Serialize;

use crate::http::shared::analytics::{AnalyticsEventBody, merge_request_meta, request_meta};
use crate::http::{AnalyticsShopper, ApiError, ApiJson, ApiResponse, ApiState};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new().route("/analytics/events", post(collect_events))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectEventsBody {
    events: Vec<AnalyticsEventBody>,
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
            let mut input = event.into_input("events.occurred_at")?;
            input.properties = merge_request_meta(input.properties, &request_meta);
            Ok::<_, ApiError>(input)
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

fn collection_result_data(result: BrowserEventCollectionResult) -> CollectionResultData {
    CollectionResultData {
        received: result.received,
        stored: result.stored,
        duplicates: result.duplicates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{
        HeaderValue,
        header::{COOKIE, USER_AGENT},
    };

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

    #[test]
    fn request_context_fills_invalid_event_matching_ids_from_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static(
                "_fbp=fb.1.1234567890123.cookie; _fbc=fb.1.1234567890123.cookie-click",
            ),
        );
        let properties = serde_json::json!({
            "_meta": {
                "fbp": "fb.1.123.invalid",
                "fbc": "fb.1.123.invalid-click"
            }
        });

        let merged = merge_request_meta(properties, &request_meta(&headers));

        assert_eq!(merged["_meta"]["fbp"], "fb.1.1234567890123.cookie");
        assert_eq!(merged["_meta"]["fbc"], "fb.1.1234567890123.cookie-click");
    }
}
