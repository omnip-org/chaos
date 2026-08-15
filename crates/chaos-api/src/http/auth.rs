use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::{delete, post},
};
use chaos_application::ApplicationError;
use chaos_application::ports::{CeremonyOptions, SessionGrant};
use chaos_domain::identity::Email;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, ApiJson, ApiResponse, ApiState, AuthenticatedSession};

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
    session: AuthenticatedSession,
) -> Result<StatusCode, ApiError> {
    state
        .passwordless_auth
        .revoke_session(&session.token)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_passkey_registration(
    State(state): State<ApiState>,
    session: AuthenticatedSession,
) -> Result<ApiResponse<CeremonyData>, ApiError> {
    let options = state
        .passwordless_auth
        .start_passkey_registration(session.user_id)
        .await?;
    Ok(ApiResponse::ok(ceremony_data(options)))
}

async fn finish_passkey_registration(
    State(state): State<ApiState>,
    session: AuthenticatedSession,
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
    let id = state
        .passwordless_auth
        .finish_passkey_registration(session.user_id, body.ceremony_id, body.credential, name)
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use chaos_application::ports::PasswordlessAuthentication;
    use chaos_domain::identity::UserId;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    use super::*;

    struct SuccessfulAuthentication {
        user_id: UserId,
    }

    #[async_trait::async_trait]
    impl PasswordlessAuthentication for SuccessfulAuthentication {
        async fn request_magic_link(&self, _email: Email) -> Result<(), ApplicationError> {
            Ok(())
        }

        async fn consume_magic_link(
            &self,
            _token: &SecretString,
        ) -> Result<SessionGrant, ApplicationError> {
            Ok(grant())
        }

        async fn authenticate_session(
            &self,
            _token: &SecretString,
        ) -> Result<UserId, ApplicationError> {
            Ok(self.user_id)
        }

        async fn revoke_session(&self, _token: &SecretString) -> Result<(), ApplicationError> {
            Ok(())
        }

        async fn start_passkey_registration(
            &self,
            _user_id: UserId,
        ) -> Result<CeremonyOptions, ApplicationError> {
            Ok(options())
        }

        async fn finish_passkey_registration(
            &self,
            _user_id: UserId,
            _ceremony_id: Uuid,
            _credential: Value,
            _name: &str,
        ) -> Result<Uuid, ApplicationError> {
            Ok(Uuid::now_v7())
        }

        async fn start_passkey_authentication(
            &self,
            _email: Email,
        ) -> Result<CeremonyOptions, ApplicationError> {
            Ok(options())
        }

        async fn finish_passkey_authentication(
            &self,
            _ceremony_id: Uuid,
            _credential: Value,
        ) -> Result<SessionGrant, ApplicationError> {
            Ok(grant())
        }
    }

    fn options() -> CeremonyOptions {
        CeremonyOptions {
            ceremony_id: Uuid::now_v7(),
            public_key: json!({ "challenge": "test" }),
        }
    }

    fn grant() -> SessionGrant {
        SessionGrant {
            token: SecretString::from("session-token".to_owned()),
            expires_in_seconds: 3600,
        }
    }

    #[tokio::test]
    async fn passwordless_http_adapter_covers_every_success_path_and_validation_boundary() {
        let user_id = UserId::new();
        let mut state = test_state("postgres://localhost/chaos", user_id);
        state.passwordless_auth = Arc::new(SuccessfulAuthentication { user_id });

        for (uri, expected) in [
            ("/admin/v1/auth/email-links", StatusCode::ACCEPTED),
            (
                "/admin/v1/auth/passkeys/authentication/options",
                StatusCode::OK,
            ),
        ] {
            let response = router(state.clone())
                .oneshot(request(
                    Method::POST,
                    uri,
                    None,
                    Some(json!({ "email": "person@example.com" })),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }

        let response = router(state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/auth/email-links/verify",
                None,
                Some(json!({ "token": "magic-token" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["token_type"],
            "Bearer"
        );

        let response = router(state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/auth/passkeys/registration/options",
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ceremony_id = Uuid::now_v7();
        let response = router(state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/auth/passkeys/registration/verify",
                None,
                Some(json!({
                    "ceremony_id": ceremony_id,
                    "name": "Laptop",
                    "credential": { "id": "credential" }
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = router(state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/auth/passkeys/authentication/verify",
                None,
                Some(json!({
                    "ceremony_id": ceremony_id,
                    "credential": { "id": "credential" }
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router(state.clone())
            .oneshot(request(
                Method::DELETE,
                "/admin/v1/auth/session",
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = router(state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/auth/email-links",
                None,
                Some(json!({ "email": "invalid" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/auth/passkeys/registration/verify",
                None,
                Some(json!({
                    "ceremony_id": ceremony_id,
                    "name": "",
                    "credential": {}
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/admin/v1/auth/passkeys/registration/options")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
