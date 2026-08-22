use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    routing::post,
};
use chaos_application::{
    ApplicationError,
    analytics::{BrowserEventCollectionResult, CollectBrowserEventsInput},
    ports::AnalyticsEventInput,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AnalyticsShopper, ApiJson, ApiResponse, ApiState, response::parse_api_time};

pub(super) fn storefront_routes() -> Router<ApiState> {
    Router::new()
        .route("/analytics/events", post(collect_events))
        .layer(DefaultBodyLimit::max(32 * 1024))
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
    ApiJson(body): ApiJson<CollectEventsBody>,
) -> Result<ApiResponse<CollectionResultData>, super::ApiError> {
    let events = body
        .events
        .into_iter()
        .map(|event| {
            Ok::<_, super::ApiError>(AnalyticsEventInput {
                event_id: event.event_id,
                event_name: event.event_name,
                occurred_at: parse_api_time(&event.occurred_at).map_err(|_| {
                    invalid_value("events.occurred_at", "must be an RFC 3339 timestamp")
                })?,
                properties: event.properties,
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

fn collection_result_data(result: BrowserEventCollectionResult) -> CollectionResultData {
    CollectionResultData {
        received: result.received,
        stored: result.stored,
        duplicates: result.duplicates,
    }
}

fn invalid_value(field: &'static str, reason: &'static str) -> super::ApiError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
    .into()
}
