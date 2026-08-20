use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::{
    ApplicationError,
    analytics::{
        BrowserEventCollectionResult, CollectBrowserEventsInput, LinkAnalyticsIdentityInput,
    },
    ports::{AnalyticsIdentityLink, IdempotencyRequest},
};
use chaos_domain::{
    FieldViolation,
    analytics::{BrowserEvent, BrowserEventProperties, ConsentSnapshot},
    catalog::{ProductId, ProductVariantId},
    sales::{CartId, CheckoutId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    AnalyticsCustomer, AnalyticsMachine, ApiDateTime, ApiError, ApiJson, ApiResponse, ApiState,
    pagination::idempotency_key, response::parse_api_time,
};

pub(super) fn storefront_routes() -> Router<ApiState> {
    Router::new()
        .route("/analytics/events", post(collect_events))
        .route("/analytics/identity-links", post(link_identity))
        .layer(DefaultBodyLimit::max(32 * 1024))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LinkIdentityBody {
    anonymous_id: Uuid,
    consent: ConsentBody,
}

#[derive(Serialize)]
struct IdentityLinkData {
    id: Uuid,
    store_id: Uuid,
    anonymous_id: Uuid,
    consent_policy_version: String,
    collection_policy_version: String,
    linked_at: ApiDateTime,
    retention_expires_at: ApiDateTime,
}

async fn link_identity(
    State(state): State<ApiState>,
    headers: HeaderMap,
    AnalyticsCustomer(actor): AnalyticsCustomer,
    ApiJson(body): ApiJson<LinkIdentityBody>,
) -> Result<ApiResponse<IdentityLinkData>, ApiError> {
    let request = fingerprinted_request(&headers, &(actor.machine.store_id.as_uuid(), &body))?;
    let consent = ConsentSnapshot::new(
        body.consent.analytics_storage,
        body.consent.advertising_storage,
        body.consent.policy_version,
    )?;
    let link = state
        .analytics_privacy
        .link_identity(LinkAnalyticsIdentityInput {
            actor,
            anonymous_id: body.anonymous_id,
            consent,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(identity_link_data(link)))
}

fn identity_link_data(item: AnalyticsIdentityLink) -> IdentityLinkData {
    IdentityLinkData {
        id: item.id,
        store_id: item.store_id.as_uuid(),
        anonymous_id: item.anonymous_id,
        consent_policy_version: item.consent_policy_version,
        collection_policy_version: item.collection_policy_version,
        linked_at: item.linked_at.into(),
        retention_expires_at: item.retention_expires_at.into(),
    }
}

fn fingerprinted_request<T: Serialize>(
    headers: &HeaderMap,
    value: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(value)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
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

#[derive(Deserialize, Serialize)]
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
    campaign_source: Option<String>,
    campaign_medium: Option<String>,
    campaign_name: Option<String>,
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
    discarded_for_policy: usize,
    collection_policy_version: String,
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
        .await
        .inspect_err(|error| {
            if matches!(error, ApplicationError::RateLimited { .. }) {
                ::metrics::counter!("chaos_analytics_collection_rate_limited_total").increment(1);
            }
        })?;
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
            BrowserEventProperties::page_viewed(
                value.path,
                value.title,
                value.referrer_domain,
                value.campaign_source,
                value.campaign_medium,
                value.campaign_name,
            )?
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
        discarded_for_policy: result.discarded_for_policy,
        collection_policy_version: result.collection_policy_version,
    }
}

fn invalid_value(field: &'static str, reason: &'static str) -> ApiError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
    .into()
}
