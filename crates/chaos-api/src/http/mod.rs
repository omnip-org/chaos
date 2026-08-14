mod auth;
mod error;
mod extract;
mod health;
mod merchant;
mod response;

use axum::Router;
use chaos_application::{
    merchant::{CreateMerchantAccount, CreateStore, MerchantQueries},
    ports::PasswordlessAuthentication,
};
use std::sync::Arc;

use chaos_infrastructure::{
    config::Settings,
    passwordless::PasswordlessAuth,
    repositories::{
        PostgresMerchantProvisioningUnitOfWork, PostgresMerchantReadRepository,
        PostgresStoreProvisioningUnitOfWork,
    },
    state::AppState,
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::lifecycle::Lifecycle;

pub use error::{ApiError, ErrorBody, ErrorDetail, ErrorEnvelope};
pub use extract::{ApiJson, ApiQuery, AuthenticatedSession, MerchantContext};
pub use response::{ApiResponse, PageMeta, ResponseEnvelope, ResponseMeta};

#[derive(Clone)]
pub struct ApiState {
    pub infrastructure: AppState,
    pub lifecycle: Lifecycle,
    pub passwordless_auth: Arc<dyn PasswordlessAuthentication>,
    pub create_merchant_account: Arc<CreateMerchantAccount>,
    pub create_store: Arc<CreateStore>,
    pub merchant_queries: Arc<MerchantQueries>,
}

impl ApiState {
    pub fn new(
        infrastructure: AppState,
        lifecycle: Lifecycle,
        settings: &Settings,
    ) -> anyhow::Result<Self> {
        let passwordless_auth = PasswordlessAuth::new(
            infrastructure.control_plane_pool(),
            infrastructure.redis_client(),
            &settings.webauthn_rp_id,
            &settings.webauthn_rp_origin,
            &settings.smtp_url,
            &settings.email_from,
            &settings.auth_public_base_url,
        )?;
        let create_merchant_account = CreateMerchantAccount::new(Arc::new(
            PostgresMerchantProvisioningUnitOfWork::new(infrastructure.runtime_pool()),
        ));
        let create_store = CreateStore::new(Arc::new(PostgresStoreProvisioningUnitOfWork::new(
            infrastructure.runtime_pool(),
        )));
        let merchant_queries = MerchantQueries::new(Arc::new(PostgresMerchantReadRepository::new(
            infrastructure.runtime_pool(),
        )));
        Ok(Self {
            infrastructure,
            lifecycle,
            passwordless_auth: Arc::new(passwordless_auth),
            create_merchant_account: Arc::new(create_merchant_account),
            create_store: Arc::new(create_store),
            merchant_queries: Arc::new(merchant_queries),
        })
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .nest("/health", health::routes())
        .nest("/admin/v1/auth", auth::routes())
        .nest("/admin/v1", merchant::routes())
        .with_state(state)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use chaos_infrastructure::{config::Settings, state::AppState};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    fn test_state() -> ApiState {
        let settings = Settings {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: "postgres://localhost/chaos".into(),
            database_control_plane_url: "postgres://localhost/chaos".into(),
            database_max_connections: 1,
            database_control_plane_max_connections: 1,
            database_acquire_timeout: Duration::from_millis(10),
            database_runtime_role: None,
            database_control_plane_role: None,
            redis_url: "redis://localhost".into(),
            webauthn_rp_id: "localhost".into(),
            webauthn_rp_origin: "http://localhost:8080".into(),
            auth_public_base_url: "http://localhost:8080".into(),
            smtp_url: "smtp://localhost:1025".into(),
            email_from: "Chaos <no-reply@localhost>".into(),
            dependency_timeout: Duration::from_millis(10),
            shutdown_drain_delay: Duration::ZERO,
            log_filter: "off".into(),
            log_json: false,
        };
        ApiState::new(
            AppState::new(&settings).unwrap(),
            Lifecycle::new(),
            &settings,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn liveness_uses_the_success_envelope_and_request_id() {
        let response = router(test_state())
            .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("x-request-id"));

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["data"]["status"],
            "ok"
        );
    }

    #[tokio::test]
    async fn draining_instance_is_immediately_not_ready() {
        let state = test_state();
        state.lifecycle.begin_draining();
        let response = router(state)
            .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 2048).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(json["error"]["code"], "service_unavailable");
    }

    #[tokio::test]
    async fn malformed_json_uses_the_error_envelope() {
        let response = router(test_state())
            .oneshot(
                Request::post("/admin/v1/auth/email-links")
                    .header("content-type", "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 2048).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_json");
    }
}
