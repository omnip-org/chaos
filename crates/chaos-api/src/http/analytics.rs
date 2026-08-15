use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    analytics::{
        BrowserEventCollectionResult, CollectBrowserEventsInput, UpdateAnalyticsPolicyInput,
    },
    ports::{IdempotencyRequest, StoreAnalyticsPolicy},
};
use chaos_domain::{
    FieldViolation,
    analytics::{BrowserEvent, BrowserEventProperties, ConsentSnapshot},
    catalog::{ProductId, ProductVariantId},
    merchant::{MerchantAccountId, StoreId},
    sales::{CartId, CheckoutId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    AnalyticsMachine, ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState,
    MerchantContext, merchant::idempotency_key, response::parse_api_time,
};

pub(super) fn storefront_routes() -> Router<ApiState> {
    Router::new()
        .route("/analytics/events", post(collect_events))
        .layer(DefaultBodyLimit::max(32 * 1024))
}

pub(super) fn admin_routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/analytics-policy",
            get(get_policy).put(update_policy),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnalyticsPolicyBody {
    behavior_collection_enabled: bool,
    advertising_exports_enabled: bool,
    identity_linking_enabled: bool,
    raw_event_retention_days: u16,
}

#[derive(Serialize)]
struct AnalyticsPolicyData {
    store_id: Uuid,
    policy_version: String,
    behavior_collection_enabled: bool,
    advertising_exports_enabled: bool,
    identity_linking_enabled: bool,
    raw_event_retention_days: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<ApiDateTime>,
}

async fn get_policy(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<AnalyticsPolicyData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let policy = state
        .analytics_administration
        .get_policy(actor, StoreId::from_uuid(path.store_id), state.clock.now())
        .await?;
    Ok(ApiResponse::ok(policy_data(policy)))
}

async fn update_policy(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<AnalyticsPolicyBody>,
) -> Result<ApiResponse<AnalyticsPolicyData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let request = IdempotencyRequest {
        key: idempotency_key(&headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(path.store_id, &body))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    };
    let policy = state
        .analytics_administration
        .update_policy(UpdateAnalyticsPolicyInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            behavior_collection_enabled: body.behavior_collection_enabled,
            advertising_exports_enabled: body.advertising_exports_enabled,
            identity_linking_enabled: body.identity_linking_enabled,
            raw_event_retention_days: body.raw_event_retention_days,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(policy_data(policy)))
}

fn policy_data(item: StoreAnalyticsPolicy) -> AnalyticsPolicyData {
    AnalyticsPolicyData {
        store_id: item.store_id.as_uuid(),
        policy_version: item.policy_version,
        behavior_collection_enabled: item.policy.behavior_collection_enabled(),
        advertising_exports_enabled: item.policy.advertising_exports_enabled(),
        identity_linking_enabled: item.policy.identity_linking_enabled(),
        raw_event_retention_days: item.policy.raw_event_retention_days(),
        created_by: item.created_by.map(|id| id.as_uuid()),
        effective_at: item.effective_at.map(ApiDateTime::from),
        created_at: item.created_at.map(ApiDateTime::from),
    }
}

fn ensure_account(actual: MerchantAccountId, requested: Uuid) -> Result<(), ApiError> {
    if actual.as_uuid() == requested {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden.into())
    }
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
        discarded_for_policy: result.discarded_for_policy,
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
