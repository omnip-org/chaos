use std::collections::HashMap;

use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};
use chaos_application::{
    ApplicationError,
    ports::{MachineActor, ShopperActor},
    store::StoreActor,
};
use chaos_domain::{FieldViolation, identity::UserId, store::StoreId};
use secrecy::SecretString;
use serde::de::DeserializeOwned;
use uuid::Uuid;

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

pub struct AuthenticatedUser {
    pub user_id: UserId,
}

pub struct StorefrontMachine(pub MachineActor);
pub struct AnalyticsShopper(pub ShopperActor);
pub struct CartMachine(pub MachineActor);
pub struct OrderLookupMachine(pub MachineActor);
pub struct CartShopper(pub ShopperActor);
pub struct PaymentShopper(pub ShopperActor);

impl FromRequestParts<ApiState> for StorefrontMachine {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers)?;
        let actor = state
            .publishable_key_authentication
            .authenticate(&token)
            .await?;
        Ok(Self(actor))
    }
}

macro_rules! storefront_machine_extractor {
    ($name:ident) => {
        impl FromRequestParts<ApiState> for $name {
            type Rejection = ApiError;

            async fn from_request_parts(
                parts: &mut Parts,
                state: &ApiState,
            ) -> Result<Self, Self::Rejection> {
                let token = bearer_token(&parts.headers)?;
                let actor = state
                    .publishable_key_authentication
                    .authenticate(&token)
                    .await?;
                Ok(Self(actor))
            }
        }
    };
}

storefront_machine_extractor!(CartMachine);
storefront_machine_extractor!(OrderLookupMachine);

macro_rules! storefront_shopper_extractor {
    ($name:ident) => {
        impl FromRequestParts<ApiState> for $name {
            type Rejection = ApiError;

            async fn from_request_parts(
                parts: &mut Parts,
                state: &ApiState,
            ) -> Result<Self, Self::Rejection> {
                let token = bearer_token(&parts.headers)?;
                let machine = state
                    .publishable_key_authentication
                    .authenticate(&token)
                    .await?;
                let credential = shopper_credential(&parts.headers)?;
                let shopper_id = state.shopper_credentials.verify(&machine, &credential)?;
                Ok(Self(ShopperActor {
                    machine,
                    shopper_id,
                }))
            }
        }
    };
}

storefront_shopper_extractor!(CartShopper);
storefront_shopper_extractor!(PaymentShopper);
storefront_shopper_extractor!(AnalyticsShopper);

impl FromRequestParts<ApiState> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers)?;
        let user_id = state.identity_auth.authenticate(&token)?;
        Ok(Self { user_id })
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

fn shopper_credential(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get("x-chaos-shopper-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or(ApplicationError::Unauthorized)?;
    Ok(SecretString::from(value.to_owned()))
}

#[derive(Clone, Copy)]
pub struct StoreContext(pub StoreActor);

impl FromRequestParts<ApiState> for StoreContext {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let session = AuthenticatedUser::from_request_parts(parts, state).await?;
        let Path(parameters) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| invalid_store_id())?;
        let store_id = parameters
            .get("store_id")
            .and_then(|value| Uuid::parse_str(value).ok())
            .map(StoreId::from_uuid)
            .ok_or_else(invalid_store_id)?;
        let actor = state
            .store_queries
            .authorize(session.user_id, store_id)
            .await?;
        Ok(Self(actor))
    }
}

fn invalid_store_id() -> ApiError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field: "store_id",
            reason: "must be a valid UUID".into(),
        }],
    }
    .into()
}
