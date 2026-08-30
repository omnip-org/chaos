use axum::{Router, extract::DefaultBodyLimit, http::HeaderMap};
use chaos_core::ApplicationError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{ApiError, ApiState};

mod email;
mod payments;

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .merge(payments::routes())
        .merge(email::routes())
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
}

#[derive(Deserialize)]
struct PaymentWebhookPath {
    provider: String,
    provider_account_id: Uuid,
}

#[derive(Deserialize)]
struct EmailWebhookPath {
    provider: String,
    provider_account_id: Uuid,
}

#[derive(Serialize)]
struct WebhookReceiptData {
    accepted: bool,
}

pub(super) fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApplicationError::Unauthorized.into())
}
