use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    routing::{delete, post},
};
use chaos_application::ApplicationError;
use chaos_application::ports::{CeremonyOptions, SessionGrant};
use chaos_domain::identity::Email;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, ApiJson, ApiResponse, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/email-links", post(request_email_link))
        .route("/email-links/verify", post(verify_email_link))
        .route("/session", delete(revoke_session))
        .route(
            "/passkeys/registration/options",
            post(start_passkey_registration),
        )
        .route(
            "/passkeys/registration/verify",
            post(finish_passkey_registration),
        )
        .route(
            "/passkeys/authentication/options",
            post(start_passkey_authentication),
        )
        .route(
            "/passkeys/authentication/verify",
            post(finish_passkey_authentication),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Deserialize)]
struct RequestEmailLinkBody {
    email: String,
}

#[derive(Deserialize)]
struct VerifyEmailLinkBody {
    token: String,
}

#[derive(Deserialize)]
struct FinishPasskeyRegistrationBody {
    ceremony_id: Uuid,
    name: String,
    credential: serde_json::Value,
}

#[derive(Deserialize)]
struct StartPasskeyAuthenticationBody {
    email: String,
}

#[derive(Deserialize)]
struct FinishPasskeyAuthenticationBody {
    ceremony_id: Uuid,
    credential: serde_json::Value,
}

#[derive(Serialize)]
struct EmptyData {}

#[derive(Serialize)]
struct CeremonyData {
    ceremony_id: Uuid,
    public_key: serde_json::Value,
}

#[derive(Serialize)]
struct PasskeyData {
    id: Uuid,
}

#[derive(Serialize)]
struct SessionData {
    session_token: String,
    token_type: &'static str,
    expires_in: u32,
}

async fn request_email_link(
    State(state): State<ApiState>,
    ApiJson(body): ApiJson<RequestEmailLinkBody>,
) -> Result<ApiResponse<EmptyData>, ApiError> {
    let email = Email::parse(body.email).map_err(ApplicationError::from)?;
    state.passwordless_auth.request_magic_link(email).await?;
    Ok(ApiResponse::new(StatusCode::ACCEPTED, EmptyData {}))
}

async fn verify_email_link(
    State(state): State<ApiState>,
    ApiJson(body): ApiJson<VerifyEmailLinkBody>,
) -> Result<ApiResponse<SessionData>, ApiError> {
    let grant = state
        .passwordless_auth
        .consume_magic_link(&SecretString::from(body.token))
        .await?;
    Ok(ApiResponse::ok(session_data(grant)))
}

async fn revoke_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    state
        .passwordless_auth
        .revoke_session(&bearer_token(&headers)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_passkey_registration(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<ApiResponse<CeremonyData>, ApiError> {
    let user_id = state
        .passwordless_auth
        .authenticate_session(&bearer_token(&headers)?)
        .await?;
    let options = state
        .passwordless_auth
        .start_passkey_registration(user_id)
        .await?;
    Ok(ApiResponse::ok(ceremony_data(options)))
}

async fn finish_passkey_registration(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<FinishPasskeyRegistrationBody>,
) -> Result<ApiResponse<PasskeyData>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "name",
                reason: "must contain 1-80 characters".into(),
            }],
        }
        .into());
    }
    let user_id = state
        .passwordless_auth
        .authenticate_session(&bearer_token(&headers)?)
        .await?;
    let id = state
        .passwordless_auth
        .finish_passkey_registration(user_id, body.ceremony_id, body.credential, name)
        .await?;
    Ok(ApiResponse::created(PasskeyData { id }))
}

async fn start_passkey_authentication(
    State(state): State<ApiState>,
    ApiJson(body): ApiJson<StartPasskeyAuthenticationBody>,
) -> Result<ApiResponse<CeremonyData>, ApiError> {
    let email = Email::parse(body.email).map_err(ApplicationError::from)?;
    let options = state
        .passwordless_auth
        .start_passkey_authentication(email)
        .await?;
    Ok(ApiResponse::ok(ceremony_data(options)))
}

async fn finish_passkey_authentication(
    State(state): State<ApiState>,
    ApiJson(body): ApiJson<FinishPasskeyAuthenticationBody>,
) -> Result<ApiResponse<SessionData>, ApiError> {
    let grant = state
        .passwordless_auth
        .finish_passkey_authentication(body.ceremony_id, body.credential)
        .await?;
    Ok(ApiResponse::ok(session_data(grant)))
}

pub(super) fn bearer_token(headers: &HeaderMap) -> Result<SecretString, ApiError> {
    let value = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or(ApplicationError::Unauthorized)?;
    Ok(SecretString::from(value.to_owned()))
}

fn ceremony_data(options: CeremonyOptions) -> CeremonyData {
    CeremonyData {
        ceremony_id: options.ceremony_id,
        public_key: options.public_key,
    }
}

fn session_data(grant: SessionGrant) -> SessionData {
    SessionData {
        session_token: grant.token.expose_secret().to_owned(),
        token_type: "Bearer",
        expires_in: grant.expires_in_seconds,
    }
}
