use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    analytics::{
        BrowserEventCollectionResult, CollectBrowserEventsInput, LinkAnalyticsIdentityInput,
        RequestAnalyticsErasureInput, UpdateAnalyticsPolicyInput,
    },
    ports::{
        AnalyticsDailyReports, AnalyticsDestinationAccount, AnalyticsDestinationConfiguration,
        AnalyticsErasureRequest, AnalyticsErasureSelector, AnalyticsErasureStatus,
        AnalyticsIdentityLink, IdempotencyRequest, StoreAnalyticsPolicy,
    },
};
use chaos_domain::{
    FieldViolation,
    analytics::{
        AnalyticsDestinationProvider, AnalyticsDestinationSecretReference, BrowserEvent,
        BrowserEventProperties, ConsentSnapshot,
    },
    catalog::{ProductId, ProductVariantId},
    merchant::{MerchantAccountId, StoreId},
    sales::{CartId, CheckoutId, CustomerId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    AnalyticsCustomer, AnalyticsMachine, ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery,
    ApiResponse, ApiState, MerchantContext, merchant::idempotency_key, response::parse_api_time,
};

pub(super) fn storefront_routes() -> Router<ApiState> {
    Router::new()
        .route("/analytics/events", post(collect_events))
        .route("/analytics/identity-links", post(link_identity))
        .layer(DefaultBodyLimit::max(32 * 1024))
}

pub(super) fn admin_routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/analytics-policy",
            get(get_policy).put(update_policy),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/analytics-erasure-requests",
            post(request_erasure),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/analytics-erasure-requests/{request_id}",
            get(get_erasure_request),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/analytics-reports/daily",
            get(list_daily_reports),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/analytics-destinations",
            get(list_destinations).put(configure_destination),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
struct ErasurePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    request_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DailyReportsQuery {
    from: String,
    to: String,
}

#[derive(Serialize)]
struct DailyBehaviorReportData {
    sales_channel_id: Uuid,
    report_date: String,
    sessions: u64,
    events: u64,
    page_views: u64,
    product_views: u64,
    searches: u64,
    cart_line_additions: u64,
    checkouts_started: u64,
    active_engagement_milliseconds: u64,
    refreshed_at: ApiDateTime,
}

#[derive(Serialize)]
struct DailyCommerceReportData {
    sales_channel_id: Uuid,
    report_date: String,
    currency: String,
    orders_created: u64,
    order_amount_minor: u64,
    payments_captured: u64,
    captured_amount_minor: u64,
    refunds_succeeded: u64,
    refunded_amount_minor: u64,
    fulfillments_shipped: u64,
    returns_completed: u64,
    refreshed_at: ApiDateTime,
}

#[derive(Serialize)]
struct DailyAttributionReportData {
    sales_channel_id: Uuid,
    report_date: String,
    attribution_model: String,
    model_version: u16,
    is_direct: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    campaign_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    campaign_medium: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    campaign_name: Option<String>,
    attributed_orders: u64,
    attributed_amount_minor: u64,
    currency: String,
    refreshed_at: ApiDateTime,
}

#[derive(Serialize)]
struct AnalyticsDailyReportsData {
    behavior: Vec<DailyBehaviorReportData>,
    commerce: Vec<DailyCommerceReportData>,
    attribution: Vec<DailyAttributionReportData>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnalyticsDestinationBody {
    provider: String,
    external_destination_reference: String,
    event_source_base_url: Option<String>,
    credential_secret_reference: String,
    enabled: bool,
}

#[derive(Serialize)]
struct AnalyticsDestinationData {
    id: Uuid,
    store_id: Uuid,
    provider: &'static str,
    external_destination_reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_source_base_url: Option<String>,
    enabled: bool,
    credentials_configured: bool,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ErasureRequestBody {
    anonymous_id: Option<Uuid>,
    customer_id: Option<Uuid>,
}

#[derive(Serialize)]
struct ErasureRequestData {
    id: Uuid,
    store_id: Uuid,
    selector_kind: &'static str,
    selector_id: Uuid,
    status: &'static str,
    requested_by: Uuid,
    behavior_events_deleted: u64,
    attribution_results_deleted: u64,
    sessions_deleted: u64,
    identity_links_deleted: u64,
    requested_at: ApiDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<ApiDateTime>,
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

async fn request_erasure(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<ErasureRequestBody>,
) -> Result<ApiResponse<ErasureRequestData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let selector = match (body.anonymous_id, body.customer_id) {
        (Some(id), None) => AnalyticsErasureSelector::Anonymous(id),
        (None, Some(id)) => AnalyticsErasureSelector::Customer(CustomerId::from_uuid(id)),
        _ => {
            return Err(invalid_value(
                "selector",
                "must contain exactly one identifier",
            ));
        }
    };
    let request = fingerprinted_request(&headers, &(path.store_id, &body))?;
    let item = state
        .analytics_privacy
        .request_erasure(RequestAnalyticsErasureInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            selector,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::new(
        axum::http::StatusCode::ACCEPTED,
        erasure_request_data(item),
    ))
}

async fn get_erasure_request(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ErasurePath>,
) -> Result<ApiResponse<ErasureRequestData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let item = state
        .analytics_privacy
        .get_erasure_request(actor, StoreId::from_uuid(path.store_id), path.request_id)
        .await?;
    Ok(ApiResponse::ok(erasure_request_data(item)))
}

async fn list_daily_reports(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<DailyReportsQuery>,
) -> Result<ApiResponse<AnalyticsDailyReportsData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let format = time::format_description::parse_borrowed::<3>("[year]-[month]-[day]")
        .expect("the Analytics report date format is valid");
    let from = time::Date::parse(&query.from, &format)
        .map_err(|_| invalid_value("from", "must be an ISO 8601 calendar date"))?;
    let to = time::Date::parse(&query.to, &format)
        .map_err(|_| invalid_value("to", "must be an ISO 8601 calendar date"))?;
    let reports = state
        .analytics_reporting
        .list_daily_reports(actor, StoreId::from_uuid(path.store_id), from, to)
        .await?;
    Ok(ApiResponse::ok(daily_reports_data(reports)))
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

async fn list_destinations(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<Vec<AnalyticsDestinationData>>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let destinations = state
        .analytics_destinations
        .list(actor, StoreId::from_uuid(path.store_id))
        .await?;
    Ok(ApiResponse::ok(
        destinations.into_iter().map(destination_data).collect(),
    ))
}

async fn configure_destination(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<AnalyticsDestinationBody>,
) -> Result<ApiResponse<AnalyticsDestinationData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let provider = AnalyticsDestinationProvider::parse(&body.provider)
        .ok_or_else(|| invalid_value("provider", "must be meta_capi or ga4"))?;
    let credential_secret_reference =
        AnalyticsDestinationSecretReference::new(body.credential_secret_reference.clone())?;
    let request = fingerprinted_request(&headers, &(path.store_id, &body))?;
    let destination = state
        .analytics_destinations
        .configure(
            actor,
            StoreId::from_uuid(path.store_id),
            AnalyticsDestinationConfiguration {
                provider,
                external_destination_reference: body.external_destination_reference,
                event_source_base_url: body.event_source_base_url,
                credential_secret_reference,
                enabled: body.enabled,
            },
            request,
            state.clock.now(),
        )
        .await?;
    Ok(ApiResponse::ok(destination_data(destination)))
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

fn destination_data(item: AnalyticsDestinationAccount) -> AnalyticsDestinationData {
    AnalyticsDestinationData {
        id: item.id,
        store_id: item.store_id.as_uuid(),
        provider: item.provider.as_str(),
        external_destination_reference: item.external_destination_reference,
        event_source_base_url: item.event_source_base_url,
        enabled: item.enabled,
        credentials_configured: item.credentials_configured,
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    }
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

fn erasure_request_data(item: AnalyticsErasureRequest) -> ErasureRequestData {
    let (selector_kind, selector_id) = match item.selector {
        AnalyticsErasureSelector::Anonymous(id) => ("anonymous", id),
        AnalyticsErasureSelector::Customer(id) => ("customer", id.as_uuid()),
    };
    ErasureRequestData {
        id: item.id,
        store_id: item.store_id.as_uuid(),
        selector_kind,
        selector_id,
        status: match item.status {
            AnalyticsErasureStatus::Pending => "pending",
            AnalyticsErasureStatus::Completed => "completed",
        },
        requested_by: item.requested_by.as_uuid(),
        behavior_events_deleted: item.behavior_events_deleted,
        attribution_results_deleted: item.attribution_results_deleted,
        sessions_deleted: item.sessions_deleted,
        identity_links_deleted: item.identity_links_deleted,
        requested_at: item.requested_at.into(),
        completed_at: item.completed_at.map(Into::into),
    }
}

fn daily_reports_data(reports: AnalyticsDailyReports) -> AnalyticsDailyReportsData {
    AnalyticsDailyReportsData {
        behavior: reports
            .behavior
            .into_iter()
            .map(|item| DailyBehaviorReportData {
                sales_channel_id: item.sales_channel_id.as_uuid(),
                report_date: item.report_date.to_string(),
                sessions: item.sessions,
                events: item.events,
                page_views: item.page_views,
                product_views: item.product_views,
                searches: item.searches,
                cart_line_additions: item.cart_line_additions,
                checkouts_started: item.checkouts_started,
                active_engagement_milliseconds: item.active_engagement_milliseconds,
                refreshed_at: item.refreshed_at.into(),
            })
            .collect(),
        commerce: reports
            .commerce
            .into_iter()
            .map(|item| DailyCommerceReportData {
                sales_channel_id: item.sales_channel_id.as_uuid(),
                report_date: item.report_date.to_string(),
                currency: item.currency,
                orders_created: item.orders_created,
                order_amount_minor: item.order_amount_minor,
                payments_captured: item.payments_captured,
                captured_amount_minor: item.captured_amount_minor,
                refunds_succeeded: item.refunds_succeeded,
                refunded_amount_minor: item.refunded_amount_minor,
                fulfillments_shipped: item.fulfillments_shipped,
                returns_completed: item.returns_completed,
                refreshed_at: item.refreshed_at.into(),
            })
            .collect(),
        attribution: reports
            .attribution
            .into_iter()
            .map(|item| DailyAttributionReportData {
                sales_channel_id: item.sales_channel_id.as_uuid(),
                report_date: item.report_date.to_string(),
                attribution_model: item.attribution_model,
                model_version: item.model_version,
                is_direct: item.is_direct,
                campaign_source: item.campaign_source,
                campaign_medium: item.campaign_medium,
                campaign_name: item.campaign_name,
                attributed_orders: item.attributed_orders,
                attributed_amount_minor: item.attributed_amount_minor,
                currency: item.currency,
                refreshed_at: item.refreshed_at.into(),
            })
            .collect(),
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
    chaos_application::ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
    .into()
}
