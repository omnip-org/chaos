use axum::{
    Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    routing::{delete, post},
};
use chaos_application::ApplicationError;
use chaos_domain::identity::{AccessKeyId, IdentityProvider};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{ApiDateTime, ApiError, ApiJson, ApiResponse, ApiState, AuthenticatedUser};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/auth/external", post(sign_in))
        .route(
            "/access-keys",
            post(create_access_key).get(list_access_keys),
        )
        .route("/access-keys/{access_key_id}", delete(revoke_access_key))
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

#[derive(Deserialize)]
struct CreateAccessKeyBody {
    name: String,
}

#[derive(Deserialize)]
struct ListAccessKeysQuery {
    cursor: Option<Uuid>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct AccessKeyCreatedData {
    id: Uuid,
    name: String,
    key_identifier: String,
    display_suffix: String,
    secret: String,
}

#[derive(Serialize)]
struct AccessKeyData {
    id: Uuid,
    name: String,
    key_identifier: String,
    display_suffix: String,
    created_at: ApiDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revoked_at: Option<ApiDateTime>,
}

#[derive(Serialize)]
struct AccessKeyPageData {
    items: Vec<AccessKeyData>,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<Uuid>,
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

async fn create_access_key(
    State(state): State<ApiState>,
    AuthenticatedUser { user_id }: AuthenticatedUser,
    ApiJson(body): ApiJson<CreateAccessKeyBody>,
) -> Result<ApiResponse<AccessKeyCreatedData>, ApiError> {
    let output = state
        .access_key_management
        .create(user_id, body.name)
        .await?;
    Ok(ApiResponse::created(AccessKeyCreatedData {
        id: output.key.id().as_uuid(),
        name: output.key.name().to_owned(),
        key_identifier: output.key_identifier,
        display_suffix: output.display_suffix,
        secret: output.plaintext.expose_secret().to_owned(),
    }))
}

async fn list_access_keys(
    State(state): State<ApiState>,
    AuthenticatedUser { user_id }: AuthenticatedUser,
    Query(query): Query<ListAccessKeysQuery>,
) -> Result<ApiResponse<AccessKeyPageData>, ApiError> {
    let page = state
        .access_key_management
        .list(
            user_id,
            query.cursor.map(AccessKeyId::from_uuid),
            query.limit.unwrap_or(20),
        )
        .await?;
    let next_cursor = page
        .has_more
        .then(|| page.items.last().map(|item| item.id.as_uuid()))
        .flatten();
    let items = page
        .items
        .into_iter()
        .map(|item| AccessKeyData {
            id: item.id.as_uuid(),
            name: item.name,
            key_identifier: item.key_identifier,
            display_suffix: item.display_suffix,
            created_at: ApiDateTime::from(item.created_at),
            last_used_at: item.last_used_at.map(ApiDateTime::from),
            revoked_at: item.revoked_at.map(ApiDateTime::from),
        })
        .collect();
    Ok(ApiResponse::ok(AccessKeyPageData {
        items,
        has_more: page.has_more,
        next_cursor,
    }))
}

async fn revoke_access_key(
    State(state): State<ApiState>,
    AuthenticatedUser { user_id }: AuthenticatedUser,
    Path(access_key_id): Path<Uuid>,
) -> Result<ApiResponse<serde_json::Value>, ApiError> {
    state
        .access_key_management
        .revoke(user_id, AccessKeyId::from_uuid(access_key_id))
        .await?;
    Ok(ApiResponse::ok(serde_json::json!({ "id": access_key_id })))
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
