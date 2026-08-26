use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use chaos_core::ApplicationError;
use serde::Serialize;
use uuid::Uuid;

use crate::http::{ApiError, ApiPath, ApiResponse, ApiState};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/webhooks/stripe/{provider_account_id}", post(receive_webhook))
        .route(
            "/webhooks/payment/{provider}/{provider_account_id}",
            post(receive_payment_webhook),
        )
        .route(
            "/webhooks/email/{provider}/{provider_account_id}",
            post(receive_email_webhook),
        )
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
}

#[derive(serde::Deserialize)]
struct WebhookPath {
    provider_account_id: Uuid,
}

#[derive(serde::Deserialize)]
struct PaymentWebhookPath {
    provider: String,
    provider_account_id: Uuid,
}

#[derive(Serialize)]
struct WebhookReceiptData {
    accepted: bool,
}

async fn receive_webhook(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<WebhookPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ApiResponse<WebhookReceiptData>, crate::http::ApiError> {
    let signature = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApplicationError::Unauthorized)?;
    let accepted = state
        .payment_service
        .receive_webhook(
            chaos_domain::stripe::StripeAccountId::from_uuid(path.provider_account_id),
            signature,
            &body,
            state.clock.now(),
        )
        .await?;
    Ok(ApiResponse::new(
        StatusCode::ACCEPTED,
        WebhookReceiptData { accepted },
    ))
}

async fn receive_payment_webhook(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<PaymentWebhookPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ApiResponse<WebhookReceiptData>, crate::http::ApiError> {
    let signature = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApplicationError::Unauthorized)?;
    let accepted = state
        .payment_service
        .receive_provider_webhook(
            &path.provider,
            path.provider_account_id,
            signature,
            &body,
            state.clock.now(),
        )
        .await?;
    Ok(ApiResponse::new(
        StatusCode::ACCEPTED,
        WebhookReceiptData { accepted },
    ))
}

async fn receive_email_webhook(
    State(state): State<ApiState>,
    Path((provider, provider_account_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ApiResponse<WebhookReceiptData>, crate::http::ApiError> {
    let message_id = required_header(&headers, "svix-id")?;
    let timestamp = required_header(&headers, "svix-timestamp")?;
    let signature = required_header(&headers, "svix-signature")?;
    let accepted = state
        .email_webhooks
        .receive(chaos_core::email::ReceiveEmailWebhook {
            provider: &provider,
            provider_account_id,
            message_id,
            timestamp,
            signature,
            payload: &body,
            received_at: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::new(
        StatusCode::ACCEPTED,
        WebhookReceiptData { accepted },
    ))
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApplicationError::Unauthorized.into())
}
