use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    payments::{
        CreatePaymentAttemptInput, CreatePaymentProviderAccountInput, CreateRefundInput,
        UpdatePaymentProviderAccountInput,
    },
    ports::{
        IdempotencyRequest, PaymentAttemptDetail, PaymentClientAction,
        PaymentProviderAccountDetail, RefundDetail,
    },
};
use chaos_domain::{
    merchant::{MerchantAccountId, StoreId},
    payments::{PaymentAttemptId, PaymentProviderAccountId},
    sales::OrderId,
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, CheckoutShopper,
    MerchantContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
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
            "/store/v1/payment-attempts/{payment_attempt_id}/client-action",
            get(get_client_action),
        )
        .route(
            "/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/payment-attempts/{payment_attempt_id}/refunds",
            post(create_refund),
        )
        .route(
            "/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/payment-provider-accounts",
            get(list_provider_accounts).post(create_provider_account),
        )
        .route(
            "/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/payment-provider-accounts/{provider_account_id}",
            get(get_provider_account).put(update_provider_account),
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

#[derive(Deserialize)]
struct ProviderCollectionPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
struct ProviderPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    provider_account_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateProviderAccountBody {
    provider: String,
    display_name: String,
    external_account_reference: String,
    credential_secret_reference: String,
    webhook_secret_reference: String,
    enabled: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UpdateProviderAccountBody {
    display_name: String,
    credential_secret_reference: String,
    webhook_secret_reference: String,
    enabled: bool,
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
struct PaymentClientActionData {
    provider: String,
    r#type: &'static str,
    public_key: String,
    client_token: String,
    account_reference: String,
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

#[derive(Serialize)]
struct PaymentProviderAccountData {
    id: Uuid,
    provider: String,
    display_name: String,
    external_account_reference: String,
    enabled: bool,
    credentials_configured: bool,
    readiness_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    readiness_checked_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    readiness_blocker_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_rotation_expires_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook_rotation_expires_at: Option<ApiDateTime>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

async fn list_provider_accounts(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ProviderCollectionPath>,
    ApiQuery(query): ApiQuery<ProviderListQuery>,
) -> Result<ApiResponse<Vec<PaymentProviderAccountData>>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let after = query
        .cursor
        .as_deref()
        .map(|value| decode_cursor(value, CursorKind::PaymentProviderAccount))
        .transpose()?;
    let limit = page_limit(query.limit)?;
    let page = state
        .payment_provider_administration
        .list(actor, StoreId::from_uuid(path.store_id), after, limit)
        .await?;
    let next_cursor = page
        .has_more
        .then(|| {
            page.items.last().map(|item| {
                encode_cursor(
                    item.account.id().as_uuid(),
                    CursorKind::PaymentProviderAccount,
                )
            })
        })
        .flatten();
    Ok(
        ApiResponse::ok(page.items.into_iter().map(provider_account_data).collect())
            .with_meta(page_meta(page.has_more, next_cursor)),
    )
}

async fn get_provider_account(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ProviderPath>,
) -> Result<ApiResponse<PaymentProviderAccountData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let value = state
        .payment_provider_administration
        .get(
            actor,
            StoreId::from_uuid(path.store_id),
            PaymentProviderAccountId::from_uuid(path.provider_account_id),
        )
        .await?;
    Ok(ApiResponse::ok(provider_account_data(value)))
}

async fn create_provider_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ProviderCollectionPath>,
    ApiJson(body): ApiJson<CreateProviderAccountBody>,
) -> Result<ApiResponse<PaymentProviderAccountData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = body_request(&headers, "create_payment_provider_account", &body)?;
    let value = state
        .payment_provider_administration
        .create(CreatePaymentProviderAccountInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            provider: body.provider,
            display_name: body.display_name,
            external_account_reference: body.external_account_reference,
            credential_secret_reference: body.credential_secret_reference,
            webhook_secret_reference: body.webhook_secret_reference,
            enabled: body.enabled,
            checked_at: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(provider_account_data(value)))
}

async fn update_provider_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ProviderPath>,
    ApiJson(body): ApiJson<UpdateProviderAccountBody>,
) -> Result<ApiResponse<PaymentProviderAccountData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = body_request(
        &headers,
        "update_payment_provider_account",
        &(path.provider_account_id, &body),
    )?;
    let value = state
        .payment_provider_administration
        .update(UpdatePaymentProviderAccountInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            id: PaymentProviderAccountId::from_uuid(path.provider_account_id),
            display_name: body.display_name,
            credential_secret_reference: body.credential_secret_reference,
            webhook_secret_reference: body.webhook_secret_reference,
            enabled: body.enabled,
            checked_at: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(provider_account_data(value)))
}

async fn create_attempt(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CheckoutShopper(actor): CheckoutShopper,
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
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<AttemptPath>,
) -> Result<ApiResponse<PaymentAttemptData>, ApiError> {
    let attempt = state
        .payment_service
        .get_attempt(&actor, PaymentAttemptId::from_uuid(path.payment_attempt_id))
        .await?;
    Ok(ApiResponse::ok(attempt_data(attempt)?))
}

async fn get_client_action(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<AttemptPath>,
) -> Result<Response, ApiError> {
    let action = state
        .payment_service
        .get_client_action(&actor, PaymentAttemptId::from_uuid(path.payment_attempt_id))
        .await?;
    let mut response = ApiResponse::ok(client_action_data(action)).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
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
    let signature_header = if path.provider == "stripe" {
        "stripe-signature"
    } else {
        "x-payment-signature"
    };
    let signature = headers
        .get(signature_header)
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

fn client_action_data(value: PaymentClientAction) -> PaymentClientActionData {
    PaymentClientActionData {
        provider: value.provider,
        r#type: value.kind,
        public_key: value.public_key.expose_secret().to_owned(),
        client_token: value.client_token.expose_secret().to_owned(),
        account_reference: value.account_reference,
    }
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

fn provider_account_data(value: PaymentProviderAccountDetail) -> PaymentProviderAccountData {
    PaymentProviderAccountData {
        id: value.account.id().as_uuid(),
        provider: value.account.provider().into(),
        display_name: value.account.display_name().into(),
        external_account_reference: value.account.external_account_reference().into(),
        enabled: value.account.enabled(),
        credentials_configured: value.credentials_configured,
        readiness_status: value.readiness_status.as_str(),
        readiness_checked_at: value.readiness_checked_at.map(Into::into),
        readiness_blocker_codes: value.readiness_blocker_codes,
        credential_rotation_expires_at: value.credential_rotation_expires_at.map(Into::into),
        webhook_rotation_expires_at: value.webhook_rotation_expires_at.map(Into::into),
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    }
}

fn ensure_account(actual: MerchantAccountId, expected: Uuid) -> Result<(), ApiError> {
    if actual.as_uuid() == expected {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden.into())
    }
}
