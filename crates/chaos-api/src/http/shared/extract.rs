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

const PUBLISHABLE_KEY_HEADER: &str = "x-chaos-publishable-key";
const SHOPPER_TOKEN_HEADER: &str = "x-chaos-shopper-token";

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
            .await
            .map_err(|error| invalid_credential(error, CredentialKind::PublishableKey))?;
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
            .await
            .map_err(|error| invalid_credential(error, CredentialKind::PublishableKey))?;
        let credential = shopper_credential(&parts.headers)?;
        let shopper_id = state
            .shopper_credentials
            .verify(&machine, &credential)
            .map_err(|error| invalid_credential(error, CredentialKind::ShopperToken))?;
        Ok(Self(ShopperActor {
            machine,
            shopper_id,
        }))
    }
}

fn publishable_key(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get(PUBLISHABLE_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| missing_credential(CredentialKind::PublishableKey))?;
    Ok(SecretString::from(value.to_owned()))
}

fn shopper_credential(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get(SHOPPER_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| missing_credential(CredentialKind::ShopperToken))?;
    Ok(SecretString::from(value.to_owned()))
}

#[derive(Clone, Copy)]
enum CredentialKind {
    PublishableKey,
    ShopperToken,
}

struct CredentialContract {
    required_code: &'static str,
    required_message: &'static str,
    invalid_code: &'static str,
    invalid_message: &'static str,
}

impl CredentialKind {
    fn contract(self) -> CredentialContract {
        match self {
            Self::PublishableKey => CredentialContract {
                required_code: "publishable_key_required",
                required_message: "a publishable key is required",
                invalid_code: "publishable_key_invalid",
                invalid_message: "the publishable key is invalid",
            },
            Self::ShopperToken => CredentialContract {
                required_code: "shopper_token_required",
                required_message: "a shopper token is required",
                invalid_code: "shopper_token_invalid",
                invalid_message: "the shopper token is invalid",
            },
        }
    }
}

fn missing_credential(kind: CredentialKind) -> ApiError {
    let contract = kind.contract();
    ApiError::Request {
        status: axum::http::StatusCode::UNAUTHORIZED,
        code: contract.required_code,
        message: contract.required_message,
    }
}

fn invalid_credential(error: ApplicationError, kind: CredentialKind) -> ApiError {
    match error {
        ApplicationError::Unauthorized => {
            let contract = kind.contract();
            ApiError::Request {
                status: axum::http::StatusCode::UNAUTHORIZED,
                code: contract.invalid_code,
                message: contract.invalid_message,
            }
        }
        error => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storefront_credentials_use_distinct_channel_and_shopper_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(PUBLISHABLE_KEY_HEADER, "pk_channel".parse().unwrap());
        headers.insert(SHOPPER_TOKEN_HEADER, "shopper.token".parse().unwrap());

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

    #[test]
    fn missing_and_invalid_credentials_have_distinct_error_codes() {
        let missing_publishable_key = missing_credential(CredentialKind::PublishableKey);
        let invalid_publishable_key = invalid_credential(
            ApplicationError::Unauthorized,
            CredentialKind::PublishableKey,
        );
        let missing_shopper_token = missing_credential(CredentialKind::ShopperToken);
        let invalid_shopper_token =
            invalid_credential(ApplicationError::Unauthorized, CredentialKind::ShopperToken);

        for (error, expected) in [
            (missing_publishable_key, "publishable_key_required"),
            (invalid_publishable_key, "publishable_key_invalid"),
            (missing_shopper_token, "shopper_token_required"),
            (invalid_shopper_token, "shopper_token_invalid"),
        ] {
            assert!(matches!(
                error,
                ApiError::Request {
                    status: axum::http::StatusCode::UNAUTHORIZED,
                    code,
                    ..
                } if code == expected
            ));
        }
    }
}
