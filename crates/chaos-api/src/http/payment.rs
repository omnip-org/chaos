use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    payments::{CreatePaymentAttemptInput, CreateRefundInput},
    ports::{IdempotencyRequest, PaymentAttemptDetail, RefundDetail},
};
use chaos_domain::{
    merchant::{MerchantAccountId, StoreId},
    payments::PaymentAttemptId,
    sales::OrderId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, CheckoutMachine,
    MerchantContext, merchant::idempotency_key,
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/store/v1/orders/{order_id}/payment-attempts",
            post(create_attempt),
        )
        .route(
            "/store/v1/payment-attempts/{payment_attempt_id}",
            get(get_attempt),
        )
        .route(
            "/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/payment-attempts/{payment_attempt_id}/refunds",
            post(create_refund),
        )
        .route("/webhooks/v1/payments/{provider}", post(receive_webhook))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Deserialize)]
struct OrderPath {
    order_id: Uuid,
}

#[derive(Deserialize)]
struct AttemptPath {
    payment_attempt_id: Uuid,
}

#[derive(Deserialize)]
struct RefundPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    payment_attempt_id: Uuid,
}

#[derive(Deserialize)]
struct WebhookPath {
    provider: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateAttemptBody {
    provider: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateRefundBody {
    amount_minor: i64,
}

#[derive(Serialize)]
struct PaymentAttemptData {
    id: Uuid,
    order_id: Uuid,
    provider: String,
    amount_minor: i64,
    currency: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct RefundData {
    id: Uuid,
    payment_attempt_id: Uuid,
    amount_minor: i64,
    currency: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct WebhookReceiptData {
    accepted: bool,
}

async fn create_attempt(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CheckoutMachine(actor): CheckoutMachine,
    ApiPath(path): ApiPath<OrderPath>,
    ApiJson(body): ApiJson<CreateAttemptBody>,
) -> Result<ApiResponse<PaymentAttemptData>, ApiError> {
    let idempotency = body_request(&headers, "create_payment_attempt", &(path.order_id, &body))?;
    let attempt = state
        .payment_service
        .create_attempt(CreatePaymentAttemptInput {
            actor,
            order_id: OrderId::from_uuid(path.order_id),
            provider: body.provider,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(attempt_data(attempt)?))
}

async fn get_attempt(
    State(state): State<ApiState>,
    CheckoutMachine(actor): CheckoutMachine,
    ApiPath(path): ApiPath<AttemptPath>,
) -> Result<ApiResponse<PaymentAttemptData>, ApiError> {
    let attempt = state
        .payment_service
        .get_attempt(&actor, PaymentAttemptId::from_uuid(path.payment_attempt_id))
        .await?;
    Ok(ApiResponse::ok(attempt_data(attempt)?))
}

async fn create_refund(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<RefundPath>,
    ApiJson(body): ApiJson<CreateRefundBody>,
) -> Result<ApiResponse<RefundData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = body_request(
        &headers,
        "create_refund",
        &(path.store_id, path.payment_attempt_id, &body),
    )?;
    let refund = state
        .payment_service
        .create_refund(CreateRefundInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            payment_attempt_id: PaymentAttemptId::from_uuid(path.payment_attempt_id),
            amount_minor: body.amount_minor,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(refund_data(refund)?))
}

async fn receive_webhook(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<WebhookPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ApiResponse<WebhookReceiptData>, ApiError> {
    let signature = headers
        .get("x-payment-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApplicationError::Unauthorized)?;
    let accepted = state
        .payment_service
        .receive_webhook(&path.provider, signature, &body, state.clock.now())
        .await?;
    Ok(ApiResponse::new(
        StatusCode::ACCEPTED,
        WebhookReceiptData { accepted },
    ))
}

fn body_request<T: Serialize>(
    headers: &HeaderMap,
    operation: &'static str,
    body: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(operation, body))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

fn attempt_data(value: PaymentAttemptDetail) -> Result<PaymentAttemptData, ApplicationError> {
    Ok(PaymentAttemptData {
        id: value.id.as_uuid(),
        order_id: value.order_id.as_uuid(),
        provider: value.provider,
        amount_minor: value.amount_minor,
        currency: value.currency.as_str().into(),
        status: value.status.as_str(),
        provider_reference: value.provider_reference,
        failure_code: value.failure_code,
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    })
}

fn refund_data(value: RefundDetail) -> Result<RefundData, ApplicationError> {
    Ok(RefundData {
        id: value.id.as_uuid(),
        payment_attempt_id: value.payment_attempt_id.as_uuid(),
        amount_minor: value.amount_minor,
        currency: value.currency.as_str().into(),
        status: value.status.as_str(),
        provider_reference: value.provider_reference,
        failure_code: value.failure_code,
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
