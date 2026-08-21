use std::{sync::Arc, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request},
};
use chaos_application::{
    ApplicationError,
    ports::{AccessTokenGrant, IdentityAuthentication},
};
use chaos_domain::identity::{IdentityProvider, UserId};
use chaos_infrastructure::{config::Settings, state::AppState};
use secrecy::SecretString;
use serde_json::Value;

use crate::lifecycle::Lifecycle;

use super::ApiState;

struct FixedSession(UserId);

#[async_trait::async_trait]
impl IdentityAuthentication for FixedSession {
    fn authenticate(&self, _token: &SecretString) -> Result<UserId, ApplicationError> {
        Ok(self.0)
    }

    async fn sign_in(
        &self,
        _provider: IdentityProvider,
        _identity_token: &SecretString,
    ) -> Result<AccessTokenGrant, ApplicationError> {
        Err(ApplicationError::Unexpected(anyhow::anyhow!(
            "unused authentication operation"
        )))
    }
}

pub(crate) fn test_state(database_url: &str, user_id: UserId) -> ApiState {
    let settings = Settings {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        database_url: database_url.into(),
        database_identity_url: database_url.into(),
        database_max_connections: 4,
        database_identity_max_connections: 1,
        database_analytics_max_connections: 1,
        database_analytics_statement_timeout: Duration::from_secs(2),
        database_acquire_timeout: Duration::from_secs(2),
        database_runtime_role: Some("chaos_runtime".into()),
        database_identity_role: None,
        redis_url: "redis://127.0.0.1:1".into(),
        auth_jwt_issuer: "https://identity.chaos.test".into(),
        auth_jwt_audience: "chaos-api".into(),
        auth_jwt_secret: SecretString::from("test-jwt-secret-that-is-at-least-32-bytes"),
        auth_jwt_lifetime_seconds: 3600,
        mcp_allowed_hosts: vec!["localhost".into()],
        google_client_id: Some("test-google-client".into()),
        apple_client_id: None,
        storefront_public_base_url: "http://localhost:4321/".parse().unwrap(),
        resend_api_base_url: "http://localhost:12112/".parse().unwrap(),
        stripe_api_base_url: "http://127.0.0.1:12111/".parse().unwrap(),
        easypost_api_base_url: "http://127.0.0.1:12113/".parse().unwrap(),
        analytics_meta_api_base_url: "http://127.0.0.1:12114/".parse().unwrap(),
        provider_secret_key: chaos_infrastructure::config::SecretKey::from_base64(
            "MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=",
        )
        .unwrap(),
        media_storage: None,
        shopper_token_active_key_id: "test".into(),
        shopper_token_active_secret: "test-shopper-token-secret-32-bytes".into(),
        shopper_token_previous_key: None,
        dependency_timeout: Duration::from_secs(1),
        shutdown_drain_delay: Duration::ZERO,
        shutdown_worker_timeout: Duration::from_secs(1),
        log_filter: "off".into(),
        log_json: false,
    };
    let mut state = ApiState::new(
        AppState::new(&settings).unwrap(),
        Lifecycle::new(),
        &settings,
    )
    .unwrap();
    state.identity_auth = Arc::new(FixedSession(user_id));
    state
}

pub(crate) async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

pub(crate) fn request(
    method: Method,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer test-session");
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap()
}
