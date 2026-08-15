use axum::{Router, extract::State, http::HeaderMap, routing::get};
use chaos_application::{ports::IdempotencyRequest, sales::ChangeOrderStatusInput};
use chaos_domain::{
    merchant::{MerchantAccountId, StoreId},
    sales::{OrderId, OrderStatus},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    ApiError, ApiPath, ApiResponse, ApiState, MerchantContext,
    merchant::idempotency_key,
    storefront_sales::{OrderData, order_data},
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/orders/{order_id}",
            get(get_order),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/orders/{order_id}/confirm",
            axum::routing::post(confirm_order),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/orders/{order_id}/cancel",
            axum::routing::post(cancel_order),
        )
}

#[derive(Deserialize)]
struct OrderPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    order_id: Uuid,
}

async fn get_order(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let order = state
        .order_management
        .get_order(
            actor,
            StoreId::from_uuid(path.store_id),
            OrderId::from_uuid(path.order_id),
        )
        .await?;
    Ok(ApiResponse::ok(order_data(order)?))
}

async fn confirm_order(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    transition(state, headers, actor, path, OrderStatus::Confirmed).await
}

async fn cancel_order(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    transition(state, headers, actor, path, OrderStatus::Cancelled).await
}

async fn transition(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::MerchantActor,
    path: OrderPath,
    target_status: OrderStatus,
) -> Result<ApiResponse<OrderData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = IdempotencyRequest {
        key: idempotency_key(&headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(path.store_id, path.order_id, target_status.as_str()))
                .map_err(|error| chaos_application::ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    };
    let order = state
        .order_management
        .change_status(ChangeOrderStatusInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            order_id: OrderId::from_uuid(path.order_id),
            target_status,
            now: OffsetDateTime::now_utc(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(order_data(order)?))
}

fn ensure_account(actual: MerchantAccountId, expected: Uuid) -> Result<(), ApiError> {
    if actual.as_uuid() == expected {
        Ok(())
    } else {
        Err(chaos_application::ApplicationError::Forbidden.into())
    }
}
