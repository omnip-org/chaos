use axum::{Router, extract::State, http::HeaderMap, routing::get};
use chaos_application::{
    ports::{AdminActor, IdempotencyRequest, OrderListFilter},
    sales::ChangeOrderStatusInput,
};
use chaos_domain::{
    merchant::{MerchantAccountId, StoreId},
    sales::{CheckoutContact, CustomerId, OrderId, OrderStatus},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiError, ApiPath, ApiQuery, ApiResponse, ApiState, MerchantContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
    storefront_sales::{OrderData, order_data},
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/orders",
            get(list_orders),
        )
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

#[derive(Deserialize)]
struct OrderCollectionPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
    status: Option<String>,
    customer_id: Option<Uuid>,
    email: Option<String>,
}

async fn list_orders(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<OrderCollectionPath>,
    ApiQuery(query): ApiQuery<OrderListQuery>,
) -> Result<ApiResponse<Vec<OrderData>>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::AdminOrder))
        .transpose()?;
    let status = query
        .status
        .as_deref()
        .map(|value| {
            OrderStatus::parse(value).ok_or_else(|| {
                chaos_application::ApplicationError::Validation {
                    violations: vec![chaos_domain::FieldViolation {
                        field: "status",
                        reason: "must be pending, confirmed, or cancelled".into(),
                    }],
                }
            })
        })
        .transpose()?;
    let email = query
        .email
        .map(|value| {
            CheckoutContact::new(value, None)
                .map(|contact| contact.email().to_owned())
                .map_err(chaos_application::ApplicationError::from)
        })
        .transpose()?;
    let limit = page_limit(query.limit)?;
    let page = state
        .order_management
        .list_orders(
            AdminActor::Merchant(actor),
            StoreId::from_uuid(path.store_id),
            after,
            limit,
            OrderListFilter {
                status,
                customer_id: query.customer_id.map(CustomerId::from_uuid),
                email,
            },
        )
        .await?;
    let next_cursor = page
        .has_more
        .then(|| {
            page.items
                .last()
                .map(|order| encode_cursor(order.id.as_uuid(), CursorKind::AdminOrder))
        })
        .flatten();
    let data = page
        .items
        .into_iter()
        .map(order_data)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor)))
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
            AdminActor::Merchant(actor),
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
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            order_id: OrderId::from_uuid(path.order_id),
            target_status,
            now: state.clock.now(),
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
