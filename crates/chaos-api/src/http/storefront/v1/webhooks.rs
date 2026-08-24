use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use chaos_core::ApplicationError;
use serde::Serialize;
use uuid::Uuid;

use crate::http::{ApiPath, ApiResponse, ApiState};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new().route("/webhooks/stripe/{provider_account_id}", post(receive_webhook))
}

#[derive(serde::Deserialize)]
struct WebhookPath {
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
