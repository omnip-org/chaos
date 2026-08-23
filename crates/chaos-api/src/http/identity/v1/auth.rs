use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    routing::post,
};
use chaos_application::ApplicationError;
use chaos_domain::identity::IdentityProvider;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{ApiError, ApiJson, ApiResponse, ApiState};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/auth/external", post(sign_in))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct SignInBody {
    provider: String,
    identity_token: String,
}

#[derive(Serialize)]
struct AccessTokenData {
    user_id: Uuid,
    access_token: String,
    token_type: &'static str,
    expires_in: u32,
}

async fn sign_in(
    State(state): State<ApiState>,
    ApiJson(body): ApiJson<SignInBody>,
) -> Result<ApiResponse<AccessTokenData>, ApiError> {
    let provider = IdentityProvider::parse(&body.provider).map_err(ApplicationError::from)?;
    let grant = state
        .identity_auth
        .sign_in(provider, &SecretString::from(body.identity_token))
        .await?;
    Ok(ApiResponse::ok(AccessTokenData {
        user_id: grant.user_id.as_uuid(),
        access_token: grant.token.expose_secret().to_owned(),
        token_type: "Bearer",
        expires_in: grant.expires_in_seconds,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::{Method, StatusCode};
    use chaos_application::{
        ApplicationError,
        ports::{AccessTokenGrant, IdentityAuthentication},
    };
    use chaos_domain::identity::UserId;
    use serde_json::json;
    use tower::ServiceExt;

    use crate::http::{
        router,
        shared::test_support::{request, response_json, test_state},
    };

    use super::*;

    struct SuccessfulAuthentication;

    #[async_trait::async_trait]
    impl IdentityAuthentication for SuccessfulAuthentication {
        async fn sign_in(
            &self,
            _provider: IdentityProvider,
            _identity_token: &SecretString,
        ) -> Result<AccessTokenGrant, ApplicationError> {
            Ok(AccessTokenGrant {
                user_id: UserId::new(),
                token: SecretString::from("access-token"),
                expires_in_seconds: 3600,
            })
        }

        fn authenticate(&self, _token: &SecretString) -> Result<UserId, ApplicationError> {
            Ok(UserId::new())
        }
    }

    #[tokio::test]
    async fn exchanges_a_supported_external_identity_for_an_access_token() {
        let mut state = test_state("postgres://localhost/chaos", UserId::new());
        state.identity_auth = Arc::new(SuccessfulAuthentication);
        let response = router(state)
            .oneshot(request(
                Method::POST,
                "/identity/v1/auth/external",
                None,
                Some(json!({
                    "provider": "google",
                    "identity_token": "provider-token"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["access_token"], "access-token");
        assert!(body["data"]["user_id"].as_str().is_some());
        assert_eq!(body["data"]["token_type"], "Bearer");
    }

    #[tokio::test]
    async fn rejects_an_unsupported_identity_provider() {
        let response = router(test_state("postgres://localhost/chaos", UserId::new()))
            .oneshot(request(
                Method::POST,
                "/identity/v1/auth/external",
                None,
                Some(json!({
                    "provider": "password",
                    "identity_token": "provider-token"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
