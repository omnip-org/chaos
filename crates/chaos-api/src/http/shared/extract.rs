use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{HeaderMap, request::Parts},
};
use chaos_core::{
    ApplicationError,
    contracts::{MachineActor, ShopperActor},
};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;

use crate::http::{ApiError, ApiState};

pub struct ApiJson<T>(pub T);
pub struct ApiPath<T>(pub T);
pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
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

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_| ApiError::Request {
                status: axum::http::StatusCode::BAD_REQUEST,
                code: "invalid_path",
                message: "one or more path parameters are invalid",
            })
    }
}

pub struct PublishableChannel(pub MachineActor);
pub struct ShopperContext(pub ShopperActor);

impl FromRequestParts<ApiState> for PublishableChannel {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let token = publishable_key(&parts.headers)?;
        let actor = state
            .publishable_key_authentication
            .authenticate(token.expose_secret())
            .await?;
        Ok(Self(actor))
    }
}

impl FromRequestParts<ApiState> for ShopperContext {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let token = publishable_key(&parts.headers)?;
        let machine = state
            .publishable_key_authentication
            .authenticate(token.expose_secret())
            .await?;
        let credential = shopper_credential(&parts.headers)?;
        let shopper_id = state.shopper_credentials.verify(&machine, &credential)?;
        Ok(Self(ShopperActor {
            machine,
            shopper_id,
        }))
    }
}

fn publishable_key(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get("x-chaos-publishable-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or(ApplicationError::Unauthorized)?;
    Ok(SecretString::from(value.to_owned()))
}

fn shopper_credential(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get("x-chaos-shopper-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or(ApplicationError::Unauthorized)?;
    Ok(SecretString::from(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storefront_credentials_use_distinct_channel_and_shopper_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-chaos-publishable-key", "pk_channel".parse().unwrap());
        headers.insert("x-chaos-shopper-token", "shopper.token".parse().unwrap());

        assert_eq!(
            publishable_key(&headers).unwrap().expose_secret(),
            "pk_channel"
        );
        assert_eq!(
            shopper_credential(&headers).unwrap().expose_secret(),
            "shopper.token"
        );
    }

    #[test]
    fn authorization_is_not_a_storefront_credential() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer legacy-token".parse().unwrap());

        assert!(publishable_key(&headers).is_err());
        assert!(shopper_credential(&headers).is_err());
    }
}
