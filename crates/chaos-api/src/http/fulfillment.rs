use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::{
    ApplicationError,
    fulfillment::{
        CreateFulfillmentInput, CreateReturnInput, TransitionFulfillmentInput,
        TransitionReturnInput,
    },
    ports::{
        FulfillmentAllocationInput, FulfillmentDetail, IdempotencyRequest, ReturnDetail,
        ReturnLineInput, ReturnReceiptInput,
    },
};
use chaos_domain::{
    catalog::ProductVariantId,
    fulfillment::{FulfillmentId, FulfillmentStatus, ReturnDisposition, ReturnId, ReturnStatus},
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
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/orders/{order_id}/returns",
            post(create_return),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/returns/{return_id}/{operation}",
            post(transition_return),
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
struct ReturnPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    return_id: Uuid,
    operation: String,
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
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
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
