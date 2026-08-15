mod api_key;
mod auth;
mod catalog;
mod error;
mod extract;
mod fulfillment;
mod health;
mod inventory;
mod merchant;
mod metrics;
mod openapi;
mod order;
mod payment;
mod pricing;
mod response;
mod store_admin;
mod storefront;
mod storefront_sales;

use axum::Router;
use chaos_application::{
    catalog::{CatalogManagement, CatalogQueries, CreateProduct},
    fulfillment::FulfillmentManagement,
    inventory::InventoryManagement,
    merchant::{
        ApiKeyAuthentication, ApiKeyManagement, CreateMerchantAccount, CreateStore,
        MerchantQueries, StoreAdministration,
    },
    payments::{PaymentService, PaymentWorkers},
    ports::{Clock, PasswordlessAuthentication, ShopperCredentialCodec},
    pricing::{CreatePriceList, PricingManagement},
    sales::{OrderManagement, StorefrontSales},
    storefront::StorefrontCatalog,
};
use std::sync::Arc;

use chaos_infrastructure::{
    clock::SystemClock,
    config::Settings,
    passwordless::PasswordlessAuth,
    repositories::{
        HmacPaymentWebhookVerifier, PostgresApiKeyRepository, PostgresCatalogManagementUnitOfWork,
        PostgresCatalogProvisioningUnitOfWork, PostgresCatalogReadRepository,
        PostgresFulfillmentRepository, PostgresInventoryRepository,
        PostgresMerchantProvisioningUnitOfWork, PostgresMerchantReadRepository,
        PostgresOrderManagementRepository, PostgresPaymentRepository,
        PostgresPricingManagementRepository, PostgresPricingProvisioningUnitOfWork,
        PostgresSearchIndexer, PostgresStoreAdministrationRepository,
        PostgresStoreProvisioningUnitOfWork, PostgresStorefrontCatalogRepository,
        PostgresStorefrontSalesRepository, SandboxPaymentProvider, SecureApiKeyMaterialGenerator,
    },
    shopper::HmacShopperCredentialCodec,
    state::AppState,
};
use metrics_exporter_prometheus::PrometheusHandle;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::lifecycle::Lifecycle;

pub use error::{ApiError, ErrorBody, ErrorDetail, ErrorEnvelope};
pub use extract::{
    ApiJson, ApiPath, ApiQuery, AuthenticatedSession, CartMachine, CartShopper, CheckoutShopper,
    MerchantContext, StorefrontMachine,
};
pub use response::{ApiDateTime, ApiResponse, PageMeta, ResponseEnvelope, ResponseMeta};

#[derive(Clone)]
pub struct ApiState {
    pub infrastructure: AppState,
    pub lifecycle: Lifecycle,
    pub metrics: PrometheusHandle,
    pub passwordless_auth: Arc<dyn PasswordlessAuthentication>,
    pub create_merchant_account: Arc<CreateMerchantAccount>,
    pub create_store: Arc<CreateStore>,
    pub store_administration: Arc<StoreAdministration>,
    pub inventory_management: Arc<InventoryManagement>,
    pub create_product: Arc<CreateProduct>,
    pub catalog_queries: Arc<CatalogQueries>,
    pub catalog_management: Arc<CatalogManagement>,
    pub create_price_list: Arc<CreatePriceList>,
    pub pricing_management: Arc<PricingManagement>,
    pub merchant_queries: Arc<MerchantQueries>,
    pub api_key_management: Arc<ApiKeyManagement>,
    pub api_key_authentication: Arc<ApiKeyAuthentication>,
    pub storefront_catalog: Arc<StorefrontCatalog>,
    pub storefront_sales: Arc<StorefrontSales>,
    pub order_management: Arc<OrderManagement>,
    pub payment_service: Arc<PaymentService>,
    pub payment_workers: Arc<PaymentWorkers>,
    pub fulfillment_management: Arc<FulfillmentManagement>,
    pub search_indexer: Arc<PostgresSearchIndexer>,
    pub clock: Arc<dyn Clock>,
    pub shopper_credentials: Arc<dyn ShopperCredentialCodec>,
}

impl ApiState {
    pub fn new(
        infrastructure: AppState,
        lifecycle: Lifecycle,
        settings: &Settings,
    ) -> anyhow::Result<Self> {
        let metrics = crate::telemetry::init_metrics()?;
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
        let store_administration = StoreAdministration::new(Arc::new(
            PostgresStoreAdministrationRepository::new(infrastructure.runtime_pool()),
        ));
        let inventory_management = InventoryManagement::new(Arc::new(
            PostgresInventoryRepository::new(infrastructure.runtime_pool()),
        ));
        let create_product = CreateProduct::new(Arc::new(
            PostgresCatalogProvisioningUnitOfWork::new(infrastructure.runtime_pool()),
        ));
        let catalog_queries = CatalogQueries::new(Arc::new(PostgresCatalogReadRepository::new(
            infrastructure.runtime_pool(),
        )));
        let catalog_management = CatalogManagement::new(Arc::new(
            PostgresCatalogManagementUnitOfWork::new(infrastructure.runtime_pool()),
        ));
        let create_price_list = CreatePriceList::new(Arc::new(
            PostgresPricingProvisioningUnitOfWork::new(infrastructure.runtime_pool()),
        ));
        let pricing_management_repository = Arc::new(PostgresPricingManagementRepository::new(
            infrastructure.runtime_pool(),
        ));
        let pricing_management = PricingManagement::new(
            pricing_management_repository.clone(),
            pricing_management_repository,
        );
        let merchant_queries = MerchantQueries::new(Arc::new(PostgresMerchantReadRepository::new(
            infrastructure.runtime_pool(),
        )));
        let api_key_repository =
            Arc::new(PostgresApiKeyRepository::new(infrastructure.runtime_pool()));
        let api_key_management = ApiKeyManagement::new(
            api_key_repository.clone(),
            Arc::new(SecureApiKeyMaterialGenerator),
        );
        let api_key_authentication = ApiKeyAuthentication::new(api_key_repository);
        let storefront_catalog = StorefrontCatalog::new(Arc::new(
            PostgresStorefrontCatalogRepository::new(infrastructure.runtime_pool()),
        ));
        let storefront_sales = StorefrontSales::new(Arc::new(
            PostgresStorefrontSalesRepository::new(infrastructure.runtime_pool()),
        ));
        let order_management = OrderManagement::new(Arc::new(
            PostgresOrderManagementRepository::new(infrastructure.runtime_pool()),
        ));
        let payment_repository = Arc::new(PostgresPaymentRepository::new(
            infrastructure.runtime_pool(),
        ));
        let payment_service = PaymentService::new(
            payment_repository.clone(),
            Arc::new(HmacPaymentWebhookVerifier::new(
                settings.payment_webhook_secret.as_bytes(),
            )?),
        );
        let payment_workers = PaymentWorkers::new(
            payment_repository.clone(),
            payment_repository,
            [Arc::new(SandboxPaymentProvider)
                as Arc<dyn chaos_application::ports::PaymentProvider>],
        );
        let fulfillment_management = FulfillmentManagement::new(Arc::new(
            PostgresFulfillmentRepository::new(infrastructure.runtime_pool()),
        ));
        let search_indexer = PostgresSearchIndexer::new(infrastructure.runtime_pool());
        let shopper_credentials = HmacShopperCredentialCodec::new(
            settings.shopper_token_active_key_id.clone(),
            settings.shopper_token_active_secret.as_bytes().to_vec(),
            settings
                .shopper_token_previous_key
                .as_ref()
                .map(|(key_id, secret)| (key_id.clone(), secret.as_bytes().to_vec())),
        )?;
        Ok(Self {
            infrastructure,
            lifecycle,
            metrics,
            passwordless_auth: Arc::new(passwordless_auth),
            create_merchant_account: Arc::new(create_merchant_account),
            create_store: Arc::new(create_store),
            store_administration: Arc::new(store_administration),
            inventory_management: Arc::new(inventory_management),
            create_product: Arc::new(create_product),
            catalog_queries: Arc::new(catalog_queries),
            catalog_management: Arc::new(catalog_management),
            create_price_list: Arc::new(create_price_list),
            pricing_management: Arc::new(pricing_management),
            merchant_queries: Arc::new(merchant_queries),
            api_key_management: Arc::new(api_key_management),
            api_key_authentication: Arc::new(api_key_authentication),
            storefront_catalog: Arc::new(storefront_catalog),
            storefront_sales: Arc::new(storefront_sales),
            order_management: Arc::new(order_management),
            payment_service: Arc::new(payment_service),
            payment_workers: Arc::new(payment_workers),
            fulfillment_management: Arc::new(fulfillment_management),
            search_indexer: Arc::new(search_indexer),
            clock: Arc::new(SystemClock),
            shopper_credentials: Arc::new(shopper_credentials),
        })
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .nest("/health", health::routes())
        .nest("/metrics", metrics::routes())
        .nest("/admin/v1/auth", auth::routes())
        .nest("/admin/v1", merchant::routes())
        .nest("/admin/v1", store_admin::routes())
        .nest("/admin/v1", inventory::routes())
        .nest("/admin/v1", order::routes())
        .nest("/admin/v1", fulfillment::routes())
        .merge(payment::routes())
        .nest("/admin/v1", catalog::routes())
        .nest("/admin/v1", pricing::routes())
        .nest("/admin/v1", api_key::routes())
        .nest("/store/v1", storefront::routes())
        .nest("/store/v1", storefront_sales::routes())
        .nest("/openapi", openapi::routes())
        .with_state(state)
        .layer(axum::middleware::from_fn(metrics::track_http_request))
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
            payment_webhook_secret: "test-payment-webhook-secret-32-bytes".into(),
            shopper_token_active_key_id: "test".into(),
            shopper_token_active_secret: "test-shopper-token-secret-32-bytes".into(),
            shopper_token_previous_key: None,
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
    async fn metrics_expose_bounded_http_request_series() {
        let app = router(test_state());
        let response = app
            .clone()
            .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; version=0.0.4; charset=utf-8"
        );

        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains("chaos_http_requests_total"));
        assert!(body.contains("method=\"GET\""));
        assert!(body.contains("route=\"/health/live\""));
        assert!(body.contains("status=\"200\""));
        assert!(body.contains("chaos_http_request_duration_seconds_bucket"));
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

    #[tokio::test]
    async fn admin_openapi_contract_is_publicly_available() {
        let response = router(test_state())
            .oneshot(
                Request::get("/openapi/admin-v1.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "application/vnd.oai.openapi+json"
        );

        let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
        let contract = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(contract["info"]["title"], "Chaos Admin API");
        assert_eq!(contract["openapi"], "3.1.0");
    }

    #[tokio::test]
    async fn store_openapi_contract_is_publicly_available() {
        let response = router(test_state())
            .oneshot(
                Request::get("/openapi/store-v1.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "application/vnd.oai.openapi+json"
        );
    }

    #[tokio::test]
    async fn storefront_catalog_rejects_requests_without_a_machine_credential() {
        let response = router(test_state())
            .oneshot(
                Request::get("/store/v1/products")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), 2048).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(json["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn price_list_administration_requires_a_human_session() {
        let account_id = uuid::Uuid::now_v7();
        let store_id = uuid::Uuid::now_v7();
        let response = router(test_state())
            .oneshot(
                Request::get(format!(
                    "/admin/v1/merchant-accounts/{account_id}/stores/{store_id}/price-lists"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
