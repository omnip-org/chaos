use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Request},
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};
use chaos_application::ApplicationError;
use chaos_domain::identity::UserId;
use secrecy::SecretString;
use serde::de::DeserializeOwned;

use super::{ApiError, ApiState};

pub struct ApiJson<T>(pub T);

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
