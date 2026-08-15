use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::{
    ApplicationError,
    fulfillment::{
        CancelPurchasedShippingLabelInput, ChangeShippingServiceStatusInput,
        CreateFulfillmentInput, CreateReturnInput, CreateShippingProviderAccountInput,
        CreateShippingServiceInput, PurchaseShippingLabelInput, QuoteShippingRatesInput,
        TransitionFulfillmentInput, TransitionReturnInput, UpdateShippingProviderAccountInput,
    },
    ports::{
        FulfillmentAllocationInput, FulfillmentDetail, IdempotencyRequest, ReturnDetail,
        ReturnLineInput, ReturnReceiptInput, ShippingAddress, ShippingLabelDetail, ShippingParcel,
        ShippingProviderAccountDetail, ShippingRateQuoteDetail, ShippingServiceDetail,
    },
};
use chaos_domain::{
    catalog::ProductVariantId,
    fulfillment::{
        FulfillmentId, FulfillmentStatus, ReturnDisposition, ReturnId, ReturnStatus,
        ShippingProviderAccountId, ShippingRateQuoteId, ShippingServiceId, ShippingServiceStatus,
    },
    inventory::InventoryLocationId,
    merchant::{MerchantAccountId, StoreId},
    sales::OrderId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, MerchantContext,
    merchant::idempotency_key,
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/orders/{order_id}/fulfillments",
            post(create_fulfillment),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/fulfillments/{fulfillment_id}/{operation}",
            post(transition_fulfillment),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/fulfillments/{fulfillment_id}/shipping-rates",
            post(quote_shipping_rates),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/fulfillments/{fulfillment_id}/shipping-label",
            post(purchase_shipping_label),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/fulfillments/{fulfillment_id}/shipping-label/cancel",
            post(cancel_shipping_label),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/orders/{order_id}/returns",
            post(create_return),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/returns/{return_id}",
            axum::routing::get(get_return),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/returns/{return_id}/{operation}",
            post(transition_return),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/shipping-services",
            post(create_shipping_service).get(list_shipping_services),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/shipping-services/{shipping_service_id}/{operation}",
            post(change_shipping_service_status),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/shipping-provider-accounts",
            post(create_shipping_provider_account).get(list_shipping_provider_accounts),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/shipping-provider-accounts/{provider_account_id}",
            axum::routing::get(get_shipping_provider_account).put(update_shipping_provider_account),
        )
        .layer(DefaultBodyLimit::max(32 * 1024))
}

#[derive(Deserialize)]
struct OrderPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    order_id: Uuid,
}

#[derive(Deserialize)]
struct FulfillmentPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    fulfillment_id: Uuid,
    operation: String,
}

#[derive(Deserialize)]
struct FulfillmentResourcePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    fulfillment_id: Uuid,
}

#[derive(Deserialize)]
struct ReturnPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    return_id: Uuid,
    operation: String,
}

#[derive(Deserialize)]
struct ReturnResourcePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    return_id: Uuid,
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
struct ShippingServicePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    shipping_service_id: Uuid,
    operation: String,
}

#[derive(Deserialize)]
struct ShippingProviderPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    provider_account_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ShippingOriginBody {
    name: String,
    company: Option<String>,
    address_line_1: String,
    address_line_2: Option<String>,
    city: String,
    region: Option<String>,
    postal_code: String,
    country_code: String,
    phone: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateShippingProviderAccountBody {
    provider: String,
    display_name: String,
    credential_secret_reference: String,
    origin: ShippingOriginBody,
    enabled: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UpdateShippingProviderAccountBody {
    display_name: String,
    credential_secret_reference: String,
    origin: ShippingOriginBody,
    enabled: bool,
}

#[derive(Serialize)]
struct ShippingProviderAccountData {
    id: Uuid,
    provider: String,
    display_name: String,
    credentials_configured: bool,
    origin: ShippingOriginBody,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_rotation_expires_at: Option<ApiDateTime>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QuoteShippingRatesBody {
    provider_account_id: Uuid,
    length_millimetres: u32,
    width_millimetres: u32,
    height_millimetres: u32,
    weight_grams: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PurchaseShippingLabelBody {
    rate_quote_id: Uuid,
}

#[derive(Serialize)]
struct ShippingRateData {
    id: Uuid,
    quote_request_id: Uuid,
    carrier: String,
    service: String,
    amount_minor: i64,
    currency: String,
    estimated_delivery_days: Option<u16>,
    guaranteed: bool,
    expires_at: ApiDateTime,
}

#[derive(Serialize)]
struct ShippingTrackingData {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_delivery_at: Option<ApiDateTime>,
    observed_at: ApiDateTime,
}

#[derive(Serialize)]
struct ShippingLabelData {
    id: Uuid,
    fulfillment_id: Uuid,
    rate_quote_id: Uuid,
    provider: String,
    carrier: String,
    tracking_number: String,
    label_url: String,
    label_media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cancellation_status: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracking: Option<ShippingTrackingData>,
    purchased_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateShippingServiceBody {
    code: String,
    name: String,
    amount_minor: i64,
    currency: String,
    estimated_min_days: u16,
    estimated_max_days: u16,
    destination_countries: Vec<String>,
}

#[derive(Serialize)]
struct ShippingServiceData {
    id: Uuid,
    code: String,
    name: String,
    amount_minor: i64,
    currency: String,
    estimated_min_days: u16,
    estimated_max_days: u16,
    destination_countries: Vec<String>,
    status: &'static str,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateFulfillmentBody {
    allocations: Vec<QuantityLine>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QuantityLine {
    product_variant_id: Uuid,
    quantity: u32,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TrackingBody {
    carrier: Option<String>,
    tracking_number: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateReturnBody {
    lines: Vec<QuantityLine>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReturnTransitionBody {
    #[serde(default)]
    receipt: Vec<ReceiptLine>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiptLine {
    product_variant_id: Uuid,
    disposition: String,
    inventory_location_id: Option<Uuid>,
}

#[derive(Serialize)]
struct FulfillmentData {
    id: Uuid,
    order_id: Uuid,
    status: &'static str,
    carrier: Option<String>,
    tracking_number: Option<String>,
    allocations: Vec<QuantityLine>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct ReturnData {
    id: Uuid,
    order_id: Uuid,
    status: &'static str,
    lines: Vec<QuantityLine>,
    refund_id: Option<Uuid>,
    refund_amount_minor: i64,
    currency: String,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

async fn create_shipping_service(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreateShippingServiceBody>,
) -> Result<ApiResponse<ShippingServiceData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = request(&headers, "create_shipping_service", &body)?;
    let detail = state
        .shipping_management
        .create(CreateShippingServiceInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            code: body.code,
            name: body.name,
            currency: body.currency,
            amount_minor: body.amount_minor,
            estimated_min_days: body.estimated_min_days,
            estimated_max_days: body.estimated_max_days,
            destination_countries: body.destination_countries,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(shipping_service_data(detail)))
}

async fn list_shipping_services(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<Vec<ShippingServiceData>>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let services = state
        .shipping_management
        .list(actor, StoreId::from_uuid(path.store_id))
        .await?;
    Ok(ApiResponse::ok(
        services.into_iter().map(shipping_service_data).collect(),
    ))
}

async fn change_shipping_service_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ShippingServicePath>,
) -> Result<ApiResponse<ShippingServiceData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let status = match path.operation.as_str() {
        "activate" => ShippingServiceStatus::Active,
        "archive" => ShippingServiceStatus::Archived,
        _ => return Err(operation_not_found(path.operation)),
    };
    let idempotency = request(
        &headers,
        "change_shipping_service_status",
        &(path.shipping_service_id, status.as_str()),
    )?;
    let detail = state
        .shipping_management
        .change_status(ChangeShippingServiceStatusInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            service_id: ShippingServiceId::from_uuid(path.shipping_service_id),
            status,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(shipping_service_data(detail)))
}

fn shipping_service_data(detail: ShippingServiceDetail) -> ShippingServiceData {
    ShippingServiceData {
        id: detail.service.id().as_uuid(),
        code: detail.service.code().into(),
        name: detail.service.name().into(),
        amount_minor: detail.service.rate().amount_minor(),
        currency: detail.service.rate().currency().as_str().into(),
        estimated_min_days: detail.service.estimated_min_days(),
        estimated_max_days: detail.service.estimated_max_days(),
        destination_countries: detail.service.destination_countries().to_vec(),
        status: detail.service.status().as_str(),
        created_at: detail.created_at.into(),
        updated_at: detail.updated_at.into(),
    }
}

async fn list_shipping_provider_accounts(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<Vec<ShippingProviderAccountData>>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let values = state
        .shipping_provider_administration
        .list(actor, StoreId::from_uuid(path.store_id))
        .await?;
    Ok(ApiResponse::ok(
        values
            .into_iter()
            .map(shipping_provider_account_data)
            .collect(),
    ))
}

async fn get_shipping_provider_account(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ShippingProviderPath>,
) -> Result<ApiResponse<ShippingProviderAccountData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let value = state
        .shipping_provider_administration
        .get(
            actor,
            StoreId::from_uuid(path.store_id),
            ShippingProviderAccountId::from_uuid(path.provider_account_id),
        )
        .await?;
    Ok(ApiResponse::ok(shipping_provider_account_data(value)))
}

async fn create_shipping_provider_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreateShippingProviderAccountBody>,
) -> Result<ApiResponse<ShippingProviderAccountData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = request(&headers, "create_shipping_provider_account", &body)?;
    let value = state
        .shipping_provider_administration
        .create(CreateShippingProviderAccountInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            provider: body.provider,
            display_name: body.display_name,
            credential_secret_reference: body.credential_secret_reference,
            origin: shipping_origin(body.origin),
            enabled: body.enabled,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(shipping_provider_account_data(value)))
}

async fn update_shipping_provider_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ShippingProviderPath>,
    ApiJson(body): ApiJson<UpdateShippingProviderAccountBody>,
) -> Result<ApiResponse<ShippingProviderAccountData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = request(
        &headers,
        "update_shipping_provider_account",
        &(path.provider_account_id, &body),
    )?;
    let value = state
        .shipping_provider_administration
        .update(UpdateShippingProviderAccountInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            id: ShippingProviderAccountId::from_uuid(path.provider_account_id),
            display_name: body.display_name,
            credential_secret_reference: body.credential_secret_reference,
            origin: shipping_origin(body.origin),
            enabled: body.enabled,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(shipping_provider_account_data(value)))
}

fn shipping_origin(value: ShippingOriginBody) -> ShippingAddress {
    ShippingAddress {
        name: value.name,
        company: value.company,
        address_line_1: value.address_line_1,
        address_line_2: value.address_line_2,
        city: value.city,
        region: value.region,
        postal_code: value.postal_code,
        country_code: value.country_code,
        phone: value.phone,
        email: value.email,
    }
}

fn shipping_provider_account_data(
    value: ShippingProviderAccountDetail,
) -> ShippingProviderAccountData {
    ShippingProviderAccountData {
        id: value.account.id().as_uuid(),
        provider: value.account.provider().into(),
        display_name: value.account.display_name().into(),
        credentials_configured: value.credentials_configured,
        origin: ShippingOriginBody {
            name: value.origin.name,
            company: value.origin.company,
            address_line_1: value.origin.address_line_1,
            address_line_2: value.origin.address_line_2,
            city: value.origin.city,
            region: value.origin.region,
            postal_code: value.origin.postal_code,
            country_code: value.origin.country_code,
            phone: value.origin.phone,
            email: value.origin.email,
        },
        enabled: value.account.enabled(),
        credential_rotation_expires_at: value.credential_rotation_expires_at.map(Into::into),
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    }
}

async fn quote_shipping_rates(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<FulfillmentResourcePath>,
    ApiJson(body): ApiJson<QuoteShippingRatesBody>,
) -> Result<ApiResponse<Vec<ShippingRateData>>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = request(
        &headers,
        "quote_shipping_rates",
        &(path.fulfillment_id, &body),
    )?;
    let rates = state
        .shipping_provider_administration
        .quote_rates(QuoteShippingRatesInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            fulfillment_id: FulfillmentId::from_uuid(path.fulfillment_id),
            provider_account_id: ShippingProviderAccountId::from_uuid(body.provider_account_id),
            parcel: ShippingParcel {
                length_millimetres: body.length_millimetres,
                width_millimetres: body.width_millimetres,
                height_millimetres: body.height_millimetres,
                weight_grams: body.weight_grams,
            },
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(
        rates.into_iter().map(shipping_rate_data).collect(),
    ))
}

async fn purchase_shipping_label(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<FulfillmentResourcePath>,
    ApiJson(body): ApiJson<PurchaseShippingLabelBody>,
) -> Result<ApiResponse<ShippingLabelData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = request(
        &headers,
        "purchase_shipping_label",
        &(path.fulfillment_id, &body),
    )?;
    let label = state
        .shipping_provider_administration
        .purchase_label(PurchaseShippingLabelInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            fulfillment_id: FulfillmentId::from_uuid(path.fulfillment_id),
            rate_quote_id: ShippingRateQuoteId::from_uuid(body.rate_quote_id),
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(shipping_label_data(label)))
}

async fn cancel_shipping_label(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<FulfillmentResourcePath>,
) -> Result<ApiResponse<ShippingLabelData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = request(&headers, "cancel_shipping_label", &path.fulfillment_id)?;
    let label = state
        .shipping_provider_administration
        .cancel_label(CancelPurchasedShippingLabelInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            fulfillment_id: FulfillmentId::from_uuid(path.fulfillment_id),
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(shipping_label_data(label)))
}

fn shipping_rate_data(value: ShippingRateQuoteDetail) -> ShippingRateData {
    ShippingRateData {
        id: value.id.as_uuid(),
        quote_request_id: value.quote_request_id.as_uuid(),
        carrier: value.rate.carrier,
        service: value.rate.service,
        amount_minor: value.rate.amount_minor,
        currency: value.rate.currency.as_str().into(),
        estimated_delivery_days: value.rate.estimated_delivery_days,
        guaranteed: value.rate.guaranteed,
        expires_at: value.expires_at.into(),
    }
}

fn shipping_label_data(value: ShippingLabelDetail) -> ShippingLabelData {
    ShippingLabelData {
        id: value.id.as_uuid(),
        fulfillment_id: value.fulfillment_id.as_uuid(),
        rate_quote_id: value.rate_quote_id.as_uuid(),
        provider: value.provider,
        carrier: value.label.carrier,
        tracking_number: value.label.tracking_number,
        label_url: value.label.label_url,
        label_media_type: value.label.label_media_type,
        cancellation_status: value.cancellation_status.map(|status| match status {
            chaos_application::ports::ShippingCancellationStatus::Submitted => "submitted",
            chaos_application::ports::ShippingCancellationStatus::Cancelled => "cancelled",
            chaos_application::ports::ShippingCancellationStatus::Rejected => "rejected",
            chaos_application::ports::ShippingCancellationStatus::NotAvailable => "not_available",
        }),
        tracking: value.tracking.map(|tracking| ShippingTrackingData {
            status: match tracking.status {
                chaos_application::ports::ProviderTrackingStatus::PreTransit => "pre_transit",
                chaos_application::ports::ProviderTrackingStatus::InTransit => "in_transit",
                chaos_application::ports::ProviderTrackingStatus::OutForDelivery => {
                    "out_for_delivery"
                }
                chaos_application::ports::ProviderTrackingStatus::Delivered => "delivered",
                chaos_application::ports::ProviderTrackingStatus::Failure => "failure",
                chaos_application::ports::ProviderTrackingStatus::Unknown => "unknown",
            },
            status_detail: tracking.status_detail,
            estimated_delivery_at: tracking.estimated_delivery_at.map(Into::into),
            observed_at: tracking.observed_at.into(),
        }),
        purchased_at: value.purchased_at.into(),
        updated_at: value.updated_at.into(),
    }
}

async fn create_fulfillment(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<OrderPath>,
    ApiJson(body): ApiJson<CreateFulfillmentBody>,
) -> Result<ApiResponse<FulfillmentData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = request(&headers, "create_fulfillment", &(path.order_id, &body))?;
    let detail = state
        .fulfillment_management
        .create_fulfillment(CreateFulfillmentInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            order_id: OrderId::from_uuid(path.order_id),
            allocations: body
                .allocations
                .into_iter()
                .map(|line| FulfillmentAllocationInput {
                    product_variant_id: ProductVariantId::from_uuid(line.product_variant_id),
                    quantity: line.quantity,
                })
                .collect(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(fulfillment_data(detail)?))
}

async fn transition_fulfillment(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<FulfillmentPath>,
    ApiJson(body): ApiJson<TrackingBody>,
) -> Result<ApiResponse<FulfillmentData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let target_status = match path.operation.as_str() {
        "ship" => FulfillmentStatus::Shipped,
        "deliver" => FulfillmentStatus::Delivered,
        "cancel" => FulfillmentStatus::Cancelled,
        _ => return Err(operation_not_found(path.operation)),
    };
    let idempotency = request(
        &headers,
        "transition_fulfillment",
        &(path.fulfillment_id, target_status.as_str(), &body),
    )?;
    let detail = state
        .fulfillment_management
        .transition_fulfillment(TransitionFulfillmentInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            fulfillment_id: FulfillmentId::from_uuid(path.fulfillment_id),
            target_status,
            carrier: body.carrier,
            tracking_number: body.tracking_number,
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(fulfillment_data(detail)?))
}

async fn create_return(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<OrderPath>,
    ApiJson(body): ApiJson<CreateReturnBody>,
) -> Result<ApiResponse<ReturnData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = request(&headers, "create_return", &(path.order_id, &body))?;
    let detail = state
        .fulfillment_management
        .create_return(CreateReturnInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            order_id: OrderId::from_uuid(path.order_id),
            lines: body
                .lines
                .into_iter()
                .map(|line| ReturnLineInput {
                    product_variant_id: ProductVariantId::from_uuid(line.product_variant_id),
                    quantity: line.quantity,
                })
                .collect(),
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(return_data(detail)?))
}

async fn get_return(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ReturnResourcePath>,
) -> Result<ApiResponse<ReturnData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let detail = state
        .fulfillment_management
        .get_return(
            actor,
            StoreId::from_uuid(path.store_id),
            ReturnId::from_uuid(path.return_id),
        )
        .await?;
    Ok(ApiResponse::ok(return_data(detail)?))
}

async fn transition_return(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ReturnPath>,
    ApiJson(body): ApiJson<ReturnTransitionBody>,
) -> Result<ApiResponse<ReturnData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let target_status = match path.operation.as_str() {
        "authorize" => ReturnStatus::Authorized,
        "reject" => ReturnStatus::Rejected,
        "receive" => ReturnStatus::Received,
        "complete" => ReturnStatus::Completed,
        _ => return Err(operation_not_found(path.operation)),
    };
    let idempotency = request(
        &headers,
        "transition_return",
        &(path.return_id, target_status.as_str(), &body),
    )?;
    let receipt = body
        .receipt
        .into_iter()
        .map(|line| {
            Ok(ReturnReceiptInput {
                product_variant_id: ProductVariantId::from_uuid(line.product_variant_id),
                disposition: ReturnDisposition::parse(&line.disposition).ok_or_else(|| {
                    ApplicationError::Conflict {
                        code: "invalid_return_disposition",
                        message: "return disposition must be restock or discard",
                    }
                })?,
                inventory_location_id: line
                    .inventory_location_id
                    .map(InventoryLocationId::from_uuid),
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    let detail = state
        .fulfillment_management
        .transition_return(TransitionReturnInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            return_id: ReturnId::from_uuid(path.return_id),
            target_status,
            receipt,
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(return_data(detail)?))
}

fn request<T: Serialize>(
    headers: &HeaderMap,
    operation: &'static str,
    value: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(operation, value))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

fn fulfillment_data(value: FulfillmentDetail) -> Result<FulfillmentData, ApplicationError> {
    Ok(FulfillmentData {
        id: value.id.as_uuid(),
        order_id: value.order_id.as_uuid(),
        status: value.status.as_str(),
        carrier: value.carrier,
        tracking_number: value.tracking_number,
        allocations: value
            .allocations
            .into_iter()
            .map(|line| QuantityLine {
                product_variant_id: line.product_variant_id.as_uuid(),
                quantity: line.quantity,
            })
            .collect(),
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    })
}

fn return_data(value: ReturnDetail) -> Result<ReturnData, ApplicationError> {
    Ok(ReturnData {
        id: value.id.as_uuid(),
        order_id: value.order_id.as_uuid(),
        status: value.status.as_str(),
        lines: value
            .lines
            .into_iter()
            .map(|line| QuantityLine {
                product_variant_id: line.product_variant_id.as_uuid(),
                quantity: line.quantity,
            })
            .collect(),
        refund_id: value.refund_id.map(|id| id.as_uuid()),
        refund_amount_minor: value.refund_amount_minor,
        currency: value.currency.as_str().into(),
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    })
}

fn ensure_account(actual: MerchantAccountId, expected: Uuid) -> Result<(), ApiError> {
    if actual.as_uuid() == expected {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden.into())
    }
}

fn operation_not_found(operation: String) -> ApiError {
    ApplicationError::NotFound {
        resource: "operation",
        id: operation,
    }
    .into()
}
