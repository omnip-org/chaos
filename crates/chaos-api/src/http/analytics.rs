use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    routing::post,
};
use chaos_application::analytics::{BrowserEventCollectionResult, CollectBrowserEventsInput};
use chaos_domain::{
    FieldViolation,
    analytics::{BrowserEvent, BrowserEventProperties, ConsentSnapshot},
    catalog::{ProductId, ProductVariantId},
    sales::{CartId, CheckoutId},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AnalyticsMachine, ApiError, ApiJson, ApiResponse, ApiState, response::parse_api_time};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/analytics/events", post(collect_events))
        .layer(DefaultBodyLimit::max(32 * 1024))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectEventsBody {
    events: Vec<BrowserEventBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserEventBody {
    event_id: Uuid,
    schema_version: u16,
    occurred_at: String,
    anonymous_id: Uuid,
    session_id: Uuid,
    consent: ConsentBody,
    event_name: BrowserEventNameBody,
    properties: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsentBody {
    analytics_storage: bool,
    advertising_storage: bool,
    policy_version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum BrowserEventNameBody {
    PageViewed,
    ProductViewed,
    SearchPerformed,
    CartLineAdded,
    CheckoutStarted,
    EngagementHeartbeat,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageViewedProperties {
    path: String,
    title: Option<String>,
    referrer_domain: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductViewedProperties {
    product_id: Uuid,
    product_variant_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchPerformedProperties {
    query: String,
    result_count: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CartLineAddedProperties {
    cart_id: Uuid,
    product_variant_id: Uuid,
    quantity: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutStartedProperties {
    cart_id: Uuid,
    checkout_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EngagementHeartbeatProperties {
    page_view_event_id: Uuid,
    active_milliseconds: u32,
}

#[derive(Serialize)]
struct CollectionResultData {
    received: usize,
    stored: usize,
    duplicates: usize,
    discarded_for_consent: usize,
    collection_policy_version: &'static str,
}

async fn collect_events(
    State(state): State<ApiState>,
    AnalyticsMachine(actor): AnalyticsMachine,
    ApiJson(body): ApiJson<CollectEventsBody>,
) -> Result<ApiResponse<CollectionResultData>, ApiError> {
    let events = body
        .events
        .into_iter()
        .map(browser_event)
        .collect::<Result<Vec<_>, _>>()?;
    let result = state
        .analytics_collection
        .collect(CollectBrowserEventsInput {
            actor,
            events,
            received_at: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(collection_result_data(result)))
}

fn browser_event(body: BrowserEventBody) -> Result<BrowserEvent, ApiError> {
    let occurred_at = parse_api_time(&body.occurred_at)
        .map_err(|_| invalid_value("events.occurred_at", "must be an RFC 3339 timestamp"))?;
    let consent = ConsentSnapshot::new(
        body.consent.analytics_storage,
        body.consent.advertising_storage,
        body.consent.policy_version,
    )?;
    let properties = match body.event_name {
        BrowserEventNameBody::PageViewed => {
            let value: PageViewedProperties = event_properties(body.properties)?;
            BrowserEventProperties::page_viewed(value.path, value.title, value.referrer_domain)?
        }
        BrowserEventNameBody::ProductViewed => {
            let value: ProductViewedProperties = event_properties(body.properties)?;
            BrowserEventProperties::product_viewed(
                ProductId::from_uuid(value.product_id),
                value.product_variant_id.map(ProductVariantId::from_uuid),
            )
        }
        BrowserEventNameBody::SearchPerformed => {
            let value: SearchPerformedProperties = event_properties(body.properties)?;
            BrowserEventProperties::search_performed(value.query, value.result_count)?
        }
        BrowserEventNameBody::CartLineAdded => {
            let value: CartLineAddedProperties = event_properties(body.properties)?;
            BrowserEventProperties::cart_line_added(
                CartId::from_uuid(value.cart_id),
                ProductVariantId::from_uuid(value.product_variant_id),
                value.quantity,
            )?
        }
        BrowserEventNameBody::CheckoutStarted => {
            let value: CheckoutStartedProperties = event_properties(body.properties)?;
            BrowserEventProperties::checkout_started(
                CartId::from_uuid(value.cart_id),
                value.checkout_id.map(CheckoutId::from_uuid),
            )
        }
        BrowserEventNameBody::EngagementHeartbeat => {
            let value: EngagementHeartbeatProperties = event_properties(body.properties)?;
            BrowserEventProperties::engagement_heartbeat(
                value.page_view_event_id,
                value.active_milliseconds,
            )?
        }
    };
    Ok(BrowserEvent::new(
        body.event_id,
        body.schema_version,
        occurred_at,
        body.anonymous_id,
        body.session_id,
        consent,
        properties,
    )?)
}

fn event_properties<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> Result<T, ApiError> {
    serde_json::from_value(value)
        .map_err(|_| invalid_value("events.properties", "must match the event-specific schema"))
}

fn collection_result_data(result: BrowserEventCollectionResult) -> CollectionResultData {
    CollectionResultData {
        received: result.received,
        stored: result.stored,
        duplicates: result.duplicates,
        discarded_for_consent: result.discarded_for_consent,
        collection_policy_version: result.collection_policy_version,
    }
}

fn invalid_value(field: &'static str, reason: &'static str) -> ApiError {
    chaos_application::ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
    .into()
}
