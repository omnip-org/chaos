use std::collections::HashMap;

use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};
use chaos_application::{ApplicationError, merchant::MerchantActor};
use chaos_domain::{FieldViolation, identity::UserId, merchant::MerchantAccountId};
use secrecy::SecretString;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::{ApiError, ApiState};

pub struct ApiJson<T>(pub T);
pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(ApiError::from_json_rejection)
    }
}

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(ApiError::from_query_rejection)
    }
}

pub struct AuthenticatedSession {
    pub user_id: UserId,
    pub token: SecretString,
}

impl FromRequestParts<ApiState> for AuthenticatedSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers)?;
        let user_id = state.passwordless_auth.authenticate_session(&token).await?;
        Ok(Self { user_id, token })
    }
}

fn bearer_token(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or(ApplicationError::Unauthorized)?;
    Ok(SecretString::from(value.to_owned()))
}

#[derive(Clone, Copy)]
pub struct MerchantContext(pub MerchantActor);

impl FromRequestParts<ApiState> for MerchantContext {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let session = AuthenticatedSession::from_request_parts(parts, state).await?;
        let Path(parameters) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| invalid_merchant_account_id())?;
        let merchant_account_id = parameters
            .get("merchant_account_id")
            .and_then(|value| Uuid::parse_str(value).ok())
            .map(MerchantAccountId::from_uuid)
            .ok_or_else(invalid_merchant_account_id)?;
        let actor = state
            .merchant_queries
            .authorize(session.user_id, merchant_account_id)
            .await?;
        Ok(Self(actor))
    }
}

fn invalid_merchant_account_id() -> ApiError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field: "merchant_account_id",
            reason: "must be a valid UUID".into(),
        }],
    }
    .into()
}
