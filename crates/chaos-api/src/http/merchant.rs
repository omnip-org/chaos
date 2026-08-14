use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::merchant::CreateMerchantAccountInput;
use chaos_application::{ApplicationError, ports::IdempotencyRequest};
use chaos_domain::FieldViolation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{ApiError, ApiResponse, ApiState, auth::bearer_token};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/merchant-accounts", post(create_merchant_account))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

const IDEMPOTENCY_KEY: &str = "idempotency-key";

#[derive(Deserialize, Serialize)]
struct CreateMerchantAccountBody {
    slug: String,
    display_name: String,
}

#[derive(Serialize)]
struct MerchantAccountData {
    id: Uuid,
}

async fn create_merchant_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<CreateMerchantAccountBody>,
) -> Result<ApiResponse<MerchantAccountData>, ApiError> {
    let idempotency_key = idempotency_key(&headers)?;
    let request_fingerprint = Sha256::digest(
        serde_json::to_vec(&body).map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .into();
    let owner_user_id = state
        .passwordless_auth
        .authenticate_session(&bearer_token(&headers)?)
        .await?;
    let output = state
        .create_merchant_account
        .execute(CreateMerchantAccountInput {
            owner_user_id,
            slug: body.slug,
            display_name: body.display_name,
            idempotency: IdempotencyRequest {
                key: idempotency_key,
                request_fingerprint,
            },
        })
        .await?;

    Ok(ApiResponse::created(MerchantAccountData {
        id: output.merchant_account_id.as_uuid(),
    }))
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let key = headers
        .get(IDEMPOTENCY_KEY)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .map(str::to_owned)
        .ok_or_else(|| ApplicationError::Validation {
            violations: vec![FieldViolation {
                field: "idempotency_key",
                reason: "must be a non-empty Idempotency-Key header of at most 255 bytes".into(),
            }],
        })?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn accepts_a_bounded_idempotency_key() {
        let mut headers = HeaderMap::new();
        headers.insert(
            IDEMPOTENCY_KEY,
            HeaderValue::from_static("create-storefront"),
        );

        assert_eq!(idempotency_key(&headers).unwrap(), "create-storefront");
    }

    #[test]
    fn rejects_a_missing_idempotency_key() {
        assert!(idempotency_key(&HeaderMap::new()).is_err());
    }
}
