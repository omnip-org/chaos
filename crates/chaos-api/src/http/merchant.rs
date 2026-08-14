use axum::{
    Router,
    extract::{DefaultBodyLimit, Path, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::merchant::{CreateMerchantAccountInput, CreateStoreInput};
use chaos_application::{ApplicationError, ports::IdempotencyRequest};
use chaos_domain::{FieldViolation, merchant::MerchantAccountId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{ApiError, ApiJson, ApiResponse, ApiState, auth::bearer_token};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/merchant-accounts", post(create_merchant_account))
        .route(
            "/merchant-accounts/{merchant_account_id}/stores",
            post(create_store),
        )
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

#[derive(Deserialize, Serialize)]
struct CreateStoreBody {
    code: String,
    name: String,
    default_currency: String,
}

#[derive(Serialize)]
struct StoreData {
    id: Uuid,
}

async fn create_merchant_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateMerchantAccountBody>,
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

async fn create_store(
    State(state): State<ApiState>,
    Path(merchant_account_id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateStoreBody>,
) -> Result<ApiResponse<StoreData>, ApiError> {
    let merchant_account_id = Uuid::parse_str(&merchant_account_id)
        .map(MerchantAccountId::from_uuid)
        .map_err(|_| ApplicationError::Validation {
            violations: vec![FieldViolation {
                field: "merchant_account_id",
                reason: "must be a valid UUID".into(),
            }],
        })?;
    let idempotency_key = idempotency_key(&headers)?;
    let request_fingerprint = Sha256::digest(
        serde_json::to_vec(&body).map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .into();
    let user_id = state
        .passwordless_auth
        .authenticate_session(&bearer_token(&headers)?)
        .await?;
    let output = state
        .create_store
        .execute(CreateStoreInput {
            user_id,
            merchant_account_id,
            code: body.code,
            name: body.name,
            default_currency: body.default_currency,
            idempotency: IdempotencyRequest {
                key: idempotency_key,
                request_fingerprint,
            },
        })
        .await?;

    Ok(ApiResponse::created(StoreData {
        id: output.store_id.as_uuid(),
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
