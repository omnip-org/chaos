use std::collections::HashMap;

use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{CustomerActor, MachineActor, ShopperActor},
};
use chaos_domain::{
    FieldViolation,
    identity::UserId,
    merchant::{ApiKeyScope, MerchantAccountId},
};
use secrecy::SecretString;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::{ApiError, ApiState};

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

pub struct AuthenticatedSession {
    pub user_id: UserId,
    pub token: SecretString,
}

pub struct StorefrontMachine(pub MachineActor);
pub struct AnalyticsMachine(pub MachineActor);
pub struct CartMachine(pub MachineActor);
pub struct OrderLookupMachine(pub MachineActor);
pub struct CartShopper(pub ShopperActor);
pub struct CheckoutShopper(pub ShopperActor);
pub struct CustomerSession(pub UserId);
pub struct CustomerMachine(pub CustomerActor);
pub struct CustomerCheckout(pub CustomerActor);
pub struct AnalyticsCustomer(pub CustomerActor);

impl FromRequestParts<ApiState> for StorefrontMachine {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers)?;
        let actor = state
            .api_key_authentication
            .authenticate(&token, &[ApiKeyScope::CatalogRead])
            .await?;
        Ok(Self(actor))
    }
}

macro_rules! storefront_machine_extractor {
    ($name:ident, $scope:expr) => {
        impl FromRequestParts<ApiState> for $name {
            type Rejection = ApiError;

            async fn from_request_parts(
                parts: &mut Parts,
                state: &ApiState,
            ) -> Result<Self, Self::Rejection> {
                let token = bearer_token(&parts.headers)?;
                let actor = state
                    .api_key_authentication
                    .authenticate(&token, &[$scope])
                    .await?;
                Ok(Self(actor))
            }
        }
    };
}

storefront_machine_extractor!(CartMachine, ApiKeyScope::CartsWrite);
storefront_machine_extractor!(AnalyticsMachine, ApiKeyScope::AnalyticsWrite);
storefront_machine_extractor!(OrderLookupMachine, ApiKeyScope::OrdersRead);

macro_rules! storefront_shopper_extractor {
    ($name:ident, $scope:expr) => {
        impl FromRequestParts<ApiState> for $name {
            type Rejection = ApiError;

            async fn from_request_parts(
                parts: &mut Parts,
                state: &ApiState,
            ) -> Result<Self, Self::Rejection> {
                let token = bearer_token(&parts.headers)?;
                let machine = state
                    .api_key_authentication
                    .authenticate(&token, &[$scope])
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

storefront_shopper_extractor!(CartShopper, ApiKeyScope::CartsWrite);
storefront_shopper_extractor!(CheckoutShopper, ApiKeyScope::CheckoutWrite);

macro_rules! customer_machine_extractor {
    ($name:ident, $scope:expr) => {
        impl FromRequestParts<ApiState> for $name {
            type Rejection = ApiError;
            async fn from_request_parts(
                parts: &mut Parts,
                state: &ApiState,
            ) -> Result<Self, Self::Rejection> {
                let token = bearer_token(&parts.headers)?;
                let machine = state
                    .api_key_authentication
                    .authenticate(&token, &[$scope])
                    .await?;
                let session = customer_session_token(&parts.headers)?;
                let user_id = state
                    .passwordless_auth
                    .authenticate_session(&session)
                    .await?;
                Ok(Self(CustomerActor { machine, user_id }))
            }
        }
    };
}

customer_machine_extractor!(CustomerMachine, ApiKeyScope::CartsWrite);
customer_machine_extractor!(CustomerCheckout, ApiKeyScope::CheckoutWrite);
customer_machine_extractor!(AnalyticsCustomer, ApiKeyScope::AnalyticsWrite);

impl FromRequestParts<ApiState> for CustomerSession {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &ApiState,
    ) -> Result<Self, Self::Rejection> {
        let session = customer_session_token(&parts.headers)?;
        Ok(Self(
            state
                .passwordless_auth
                .authenticate_session(&session)
                .await?,
        ))
    }
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

fn shopper_credential(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get("x-chaos-shopper-token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or(ApplicationError::Unauthorized)?;
    Ok(SecretString::from(value.to_owned()))
}

fn customer_session_token(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get("x-chaos-customer-session")
        .and_then(|value| value.to_str().ok())
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
