use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};

use crate::http::{ApiPath, ApiResponse, ApiState};

use super::{EmailWebhookPath, WebhookReceiptData, required_header};

pub(super) fn routes() -> Router<ApiState> {
    Router::new().route(
        "/email/{provider}/{provider_account_id}",
        post(receive_email_webhook),
    )
}

async fn receive_email_webhook(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<EmailWebhookPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ApiResponse<WebhookReceiptData>, crate::http::ApiError> {
    let message_id = required_header(&headers, "svix-id")?;
    let timestamp = required_header(&headers, "svix-timestamp")?;
    let signature = required_header(&headers, "svix-signature")?;
    let accepted = state
        .email_webhooks
        .receive(chaos_core::email::ReceiveEmailWebhook {
            provider: &path.provider,
            provider_account_id: path.provider_account_id,
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
