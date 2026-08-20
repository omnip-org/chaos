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
    ports::{IdempotencyRequest, VisitorCustomerLink},
};
use chaos_domain::{
    FieldViolation,
    analytics::{
        BrowserCollectionBasis, BrowserEvent, BrowserEventProperties, ConsentSnapshot,
        TrafficAttribution, TrafficTouchpoint,
    },
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
    visitor_id: Uuid,
    consent: ConsentBody,
    collection_basis: BrowserCollectionBasisBody,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum BrowserCollectionBasisBody {
    Consent,
    StorePolicy,
}

#[derive(Serialize)]
struct IdentityLinkData {
    id: Uuid,
    store_id: Uuid,
    visitor_id: Uuid,
    consent_policy_version: String,
    collection_basis: &'static str,
    settings_revision: i32,
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
            visitor_id: body.visitor_id,
            consent,
            collection_basis: match body.collection_basis {
                BrowserCollectionBasisBody::Consent => BrowserCollectionBasis::Consent,
                BrowserCollectionBasisBody::StorePolicy => BrowserCollectionBasis::StorePolicy,
            },
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(identity_link_data(link)))
}

fn identity_link_data(item: VisitorCustomerLink) -> IdentityLinkData {
    IdentityLinkData {
        id: item.id,
        store_id: item.store_id.as_uuid(),
        visitor_id: item.visitor_id,
        consent_policy_version: item.consent_policy_version,
        collection_basis: item.collection_basis.as_str(),
        settings_revision: item.settings_revision,
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
    visitor_id: Uuid,
    session_id: Uuid,
    consent: ConsentBody,
    collection_basis: BrowserCollectionBasisBody,
    traffic: Option<TrafficAttributionBody>,
    event_name: BrowserEventNameBody,
    properties: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrafficAttributionBody {
    first: TrafficTouchpointBody,
    session: TrafficTouchpointBody,
    last_non_direct: Option<TrafficTouchpointBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrafficTouchpointBody {
    source: Option<String>,
    medium: Option<String>,
    campaign: Option<String>,
    campaign_id: Option<String>,
    term: Option<String>,
    content: Option<String>,
    referrer_domain: Option<String>,
    fbclid: Option<String>,
    gclid: Option<String>,
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
    PageView,
    ViewContent,
    Search,
    AddToCart,
    InitiateCheckout,
    ViewDuration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageViewProperties {
    path: String,
    title: Option<String>,
    referrer_domain: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewContentProperties {
    product_id: Uuid,
    product_variant_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchProperties {
    query: String,
    result_count: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddToCartProperties {
    cart_id: Uuid,
    product_variant_id: Uuid,
    quantity: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitiateCheckoutProperties {
    cart_id: Uuid,
    checkout_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewDurationProperties {
    page_view_event_id: Uuid,
    active_milliseconds: u32,
}

#[derive(Serialize)]
struct CollectionResultData {
    received: usize,
    stored: usize,
    duplicates: usize,
    discarded_for_consent: usize,
    discarded_for_settings: usize,
    settings_revision: i32,
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
    let traffic = body.traffic.map(traffic_attribution).transpose()?;
    let properties = match body.event_name {
        BrowserEventNameBody::PageView => {
            let value: PageViewProperties = event_properties(body.properties)?;
            BrowserEventProperties::page_view(value.path, value.title, value.referrer_domain)?
        }
        BrowserEventNameBody::ViewContent => {
            let value: ViewContentProperties = event_properties(body.properties)?;
            BrowserEventProperties::view_content(
                ProductId::from_uuid(value.product_id),
                value.product_variant_id.map(ProductVariantId::from_uuid),
            )
        }
        BrowserEventNameBody::Search => {
            let value: SearchProperties = event_properties(body.properties)?;
            BrowserEventProperties::search(value.query, value.result_count)?
        }
        BrowserEventNameBody::AddToCart => {
            let value: AddToCartProperties = event_properties(body.properties)?;
            BrowserEventProperties::add_to_cart(
                CartId::from_uuid(value.cart_id),
                ProductVariantId::from_uuid(value.product_variant_id),
                value.quantity,
            )?
        }
        BrowserEventNameBody::InitiateCheckout => {
            let value: InitiateCheckoutProperties = event_properties(body.properties)?;
            BrowserEventProperties::initiate_checkout(
                CartId::from_uuid(value.cart_id),
                value.checkout_id.map(CheckoutId::from_uuid),
            )
        }
        BrowserEventNameBody::ViewDuration => {
            let value: ViewDurationProperties = event_properties(body.properties)?;
            BrowserEventProperties::view_duration(
                value.page_view_event_id,
                value.active_milliseconds,
            )?
        }
    };
    Ok(BrowserEvent::new(
        body.event_id,
        body.schema_version,
        occurred_at,
        body.visitor_id,
        body.session_id,
        consent,
        match body.collection_basis {
            BrowserCollectionBasisBody::Consent => BrowserCollectionBasis::Consent,
            BrowserCollectionBasisBody::StorePolicy => BrowserCollectionBasis::StorePolicy,
        },
        traffic,
        properties,
    )?)
}

fn traffic_attribution(value: TrafficAttributionBody) -> Result<TrafficAttribution, ApiError> {
    Ok(TrafficAttribution::new(
        traffic_touchpoint(value.first)?,
        traffic_touchpoint(value.session)?,
        value.last_non_direct.map(traffic_touchpoint).transpose()?,
    ))
}

fn traffic_touchpoint(value: TrafficTouchpointBody) -> Result<TrafficTouchpoint, ApiError> {
    Ok(TrafficTouchpoint::new(
        value.source,
        value.medium,
        value.campaign,
        value.campaign_id,
        value.term,
        value.content,
        value.referrer_domain,
        value.fbclid,
        value.gclid,
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
        discarded_for_settings: result.discarded_for_settings,
        settings_revision: result.settings_revision,
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
