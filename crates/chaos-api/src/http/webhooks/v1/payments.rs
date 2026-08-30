use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use chaos_core::ApplicationError;

use crate::http::{ApiPath, ApiResponse, ApiState};

use super::{PaymentWebhookPath, WebhookReceiptData};

pub(super) fn routes() -> Router<ApiState> {
    Router::new().route(
        "/payment/{provider}/{provider_account_id}",
        post(receive_payment_webhook),
    )
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
