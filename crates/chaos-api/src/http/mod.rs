mod health;
mod identity;
mod shared;
mod storefront;

use axum::Router;
use chaos_application::{
    analytics::{AnalyticsAdministration, AnalyticsCollection},
    catalog::{
        CatalogManagement, CatalogQueries, CollectionAdministration, CreateProduct,
        MediaAdministration, ReviewAdministration, StorefrontCollections, StorefrontReviews,
    },
    identity::{AccessKeyAuthentication, AccessKeyManagement, IdentityService},
    inventory::InventoryManagement,
    ports::{Clock, IdentityAuthentication, MediaStorage, ShopperCredentialCodec},
    pricing::{CreatePriceList, PricingManagement},
    sales::{OrderManagement, StorefrontSales},
    store::{
        CreateStore, ProviderSecretManagement, PublishableKeyAuthentication,
        PublishableKeyManagement, StoreAdministration, StoreMembershipManagement, StoreQueries,
    },
    storefront::StorefrontCatalog,
    stripe::{PaymentService, StripeAccountAdministration},
};
use std::sync::Arc;

use chaos_infrastructure::{
    integrations::{
        analytics::rate_limit::RedisAnalyticsCollectionRateLimiter,
        stripe::{StripeGateway, StripeWebhookVerifier},
    },
    repositories::{
        DefaultPublishableKeyGenerator, PostgresAnalyticsDestinationStore,
        PostgresAnalyticsEventStore, PostgresCatalogManagementUnitOfWork,
        PostgresCatalogProvisioningUnitOfWork, PostgresCatalogReadRepository,
        PostgresCollectionRepository, PostgresInventoryRepository, PostgresMediaAssetRepository,
        PostgresOrderManagementRepository, PostgresPricingManagementRepository,
        PostgresPricingProvisioningUnitOfWork, PostgresPublishableKeyRepository,
        PostgresReviewRepository, PostgresStoreAdministrationRepository,
        PostgresStoreMembershipRepository, PostgresStoreProvisioningUnitOfWork,
        PostgresStoreReadRepository, PostgresStorefrontCatalogRepository,
        PostgresStorefrontSalesRepository, PostgresStripeRepository,
    },
    runtime::{clock::SystemClock, config::Settings, state::AppState},
    security::{
        identity::{
            JwtAccessTokenCodec, OidcIdentityVerifier, OidcProviderConfiguration,
            PostgresAccessKeyRepository, PostgresIdentityRepository,
            SecureAccessKeyMaterialGenerator,
        },
        provider_secrets::DynamicSecretResolver,
        shopper::HmacShopperCredentialCodec,
    },
    storage::media::{S3MediaStorage, S3MediaStorageConfiguration, UnavailableMediaStorage},
};
use secrecy::ExposeSecret as _;
use tower_http::trace::TraceLayer;

use chaos_infrastructure::runtime::lifecycle::Lifecycle;

pub use shared::error::{ApiError, ErrorBody, ErrorDetail, ErrorEnvelope};
pub use shared::extract::{
    AnalyticsShopper, ApiJson, ApiPath, ApiQuery, AuthenticatedUser, CartMachine, CartShopper,
    OrderLookupMachine, PaymentShopper, StoreContext, StorefrontMachine,
};
pub use shared::response::{ApiDateTime, ApiResponse, PageMeta, ResponseEnvelope, ResponseMeta};

#[derive(Clone)]
pub struct ApiState {
    pub infrastructure: AppState,
    pub lifecycle: Lifecycle,
    pub public_base_url: String,
    pub identity_auth: Arc<dyn IdentityAuthentication>,
    pub access_key_management: Arc<AccessKeyManagement>,
    pub access_key_authentication: Arc<AccessKeyAuthentication>,
    pub mcp_allowed_hosts: Vec<String>,
    pub create_store: Arc<CreateStore>,
    pub store_administration: Arc<StoreAdministration>,
    pub inventory_management: Arc<InventoryManagement>,
    pub create_product: Arc<CreateProduct>,
    pub catalog_queries: Arc<CatalogQueries>,
    pub catalog_management: Arc<CatalogManagement>,
    pub collection_administration: Arc<CollectionAdministration>,
    pub storefront_collections: Arc<StorefrontCollections>,
    pub review_administration: Arc<ReviewAdministration>,
    pub storefront_reviews: Arc<StorefrontReviews>,
    pub media_administration: Arc<MediaAdministration>,
    pub create_price_list: Arc<CreatePriceList>,
    pub pricing_management: Arc<PricingManagement>,
    pub store_queries: Arc<StoreQueries>,
    pub store_membership_management: Arc<StoreMembershipManagement>,
    pub publishable_key_management: Arc<PublishableKeyManagement>,
    pub publishable_key_authentication: Arc<PublishableKeyAuthentication>,
    pub provider_secret_management: Arc<ProviderSecretManagement>,
    pub analytics_collection: Arc<AnalyticsCollection>,
    pub analytics_administration: Arc<AnalyticsAdministration>,
    pub storefront_catalog: Arc<StorefrontCatalog>,
    pub storefront_sales: Arc<StorefrontSales>,
    pub order_management: Arc<OrderManagement>,
    pub payment_service: Arc<PaymentService>,
    pub stripe_account_administration: Arc<StripeAccountAdministration>,
    pub clock: Arc<dyn Clock>,
    pub shopper_credentials: Arc<dyn ShopperCredentialCodec>,
}

impl ApiState {
    pub fn new(
        infrastructure: AppState,
        lifecycle: Lifecycle,
        settings: &Settings,
    ) -> anyhow::Result<Self> {
        let identity_providers = [
            settings
                .google_client_id
                .as_ref()
                .map(|audience| OidcProviderConfiguration {
                    provider: chaos_domain::identity::IdentityProvider::Google,
                    issuers: vec![
                        "https://accounts.google.com".into(),
                        "accounts.google.com".into(),
                    ],
                    audience: audience.clone(),
                    jwks_uri: "https://www.googleapis.com/oauth2/v3/certs"
                        .parse()
                        .unwrap(),
                }),
            settings
                .apple_client_id
                .as_ref()
                .map(|audience| OidcProviderConfiguration {
                    provider: chaos_domain::identity::IdentityProvider::Apple,
                    issuers: vec!["https://appleid.apple.com".into()],
                    audience: audience.clone(),
                    jwks_uri: "https://appleid.apple.com/auth/keys".parse().unwrap(),
                }),
        ]
        .into_iter()
        .flatten();
        let identity_auth = IdentityService::new(
            Arc::new(OidcIdentityVerifier::new(
                identity_providers,
                settings.dependency_timeout,
            )?),
            Arc::new(PostgresIdentityRepository::new(
                infrastructure.identity_pool(),
            )),
            Arc::new(JwtAccessTokenCodec::new(
                settings.auth_jwt_issuer.clone(),
                settings.auth_jwt_audience.clone(),
                settings.auth_jwt_secret.expose_secret().as_bytes(),
                settings.auth_jwt_lifetime_seconds,
            )?),
        );
        let access_key_repository = Arc::new(PostgresAccessKeyRepository::new(
            infrastructure.identity_pool(),
        ));
        let access_key_management = AccessKeyManagement::new(
            access_key_repository.clone(),
            Arc::new(SecureAccessKeyMaterialGenerator),
        );
        let access_key_authentication = AccessKeyAuthentication::new(access_key_repository);
        let create_store = CreateStore::new(Arc::new(PostgresStoreProvisioningUnitOfWork::new(
            infrastructure.runtime_pool(),
        )));
        let store_administration_repository = Arc::new(PostgresStoreAdministrationRepository::new(
            infrastructure.runtime_pool(),
        ));
        let store_administration =
            StoreAdministration::new(store_administration_repository.clone());
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
        let collection_repository = Arc::new(PostgresCollectionRepository::new(
            infrastructure.runtime_pool(),
        ));
        let collection_administration =
            CollectionAdministration::new(collection_repository.clone());
        let storefront_collections = StorefrontCollections::new(collection_repository);
        let review_repository =
            Arc::new(PostgresReviewRepository::new(infrastructure.runtime_pool()));
        let review_administration = ReviewAdministration::new(review_repository.clone());
        let storefront_reviews = StorefrontReviews::new(review_repository);
        let media_storage: Arc<dyn MediaStorage> =
            if let Some(configuration) = &settings.media_storage {
                Arc::new(S3MediaStorage::new(S3MediaStorageConfiguration {
                    endpoint_url: configuration.endpoint_url.clone(),
                    region: configuration.region.clone(),
                    bucket: configuration.bucket.clone(),
                    access_key_id: configuration.access_key_id.clone(),
                    secret_access_key: configuration.secret_access_key.clone(),
                    session_token: configuration.session_token.clone(),
                    force_path_style: configuration.force_path_style,
                    public_base_url: configuration.public_base_url.clone(),
                })?)
            } else {
                Arc::new(UnavailableMediaStorage)
            };
        let media_administration = MediaAdministration::new(
            Arc::new(PostgresMediaAssetRepository::new(
                infrastructure.runtime_pool(),
            )),
            media_storage,
        );
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
        let store_queries = StoreQueries::new(Arc::new(PostgresStoreReadRepository::new(
            infrastructure.runtime_pool(),
        )));
        let store_membership_management = StoreMembershipManagement::new(Arc::new(
            PostgresStoreMembershipRepository::new(infrastructure.runtime_pool()),
        ));
        let publishable_key_repository = Arc::new(PostgresPublishableKeyRepository::new(
            infrastructure.runtime_pool(),
        ));
        let publishable_key_management = PublishableKeyManagement::new(
            publishable_key_repository.clone(),
            Arc::new(DefaultPublishableKeyGenerator),
        );
        let publishable_key_authentication =
            PublishableKeyAuthentication::new(publishable_key_repository);
        let analytics_event_store = Arc::new(PostgresAnalyticsEventStore::new(
            infrastructure.runtime_pool(),
        ));
        let analytics_collection = AnalyticsCollection::new(
            analytics_event_store.clone(),
            Arc::new(RedisAnalyticsCollectionRateLimiter::new(
                infrastructure.redis_client(),
            )),
        );
        let analytics_administration = AnalyticsAdministration::new(
            Arc::new(PostgresAnalyticsDestinationStore::new(
                infrastructure.runtime_pool(),
            )),
            analytics_event_store,
        );
        let dynamic_secrets = Arc::new(DynamicSecretResolver::new(&settings.provider_secret_key));
        let provider_secret_management =
            ProviderSecretManagement::new(store_administration_repository, dynamic_secrets.clone());
        let storefront_catalog = StorefrontCatalog::new(Arc::new(
            PostgresStorefrontCatalogRepository::new(infrastructure.runtime_pool()),
        ));
        let storefront_sales = StorefrontSales::new(Arc::new(
            PostgresStorefrontSalesRepository::new(infrastructure.runtime_pool()),
        ));
        let order_management = OrderManagement::new(Arc::new(
            PostgresOrderManagementRepository::new(infrastructure.runtime_pool()),
        ));
        let payment_repository =
            Arc::new(PostgresStripeRepository::new(infrastructure.runtime_pool()));
        let payment_secrets = dynamic_secrets.clone();
        let stripe_gateway = Arc::new(StripeGateway::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            payment_secrets.clone(),
        )?);
        let payment_provider =
            stripe_gateway.clone() as Arc<dyn chaos_application::ports::StripePaymentGateway>;
        let payment_onboarding =
            stripe_gateway.clone() as Arc<dyn chaos_application::ports::StripeAccountReadiness>;
        let webhook_verifier = Arc::new(StripeWebhookVerifier::new(
            payment_repository.clone(),
            payment_secrets,
        ))
            as Arc<dyn chaos_application::ports::StripeWebhookSignatureVerifier>;
        let payment_service = PaymentService::new(
            payment_repository.clone(),
            webhook_verifier,
            payment_provider,
        );
        let stripe_account_administration =
            StripeAccountAdministration::new(payment_repository.clone(), payment_onboarding);
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
            public_base_url: settings.public_base_url.to_string(),
            identity_auth: Arc::new(identity_auth),
            access_key_management: Arc::new(access_key_management),
            access_key_authentication: Arc::new(access_key_authentication),
            mcp_allowed_hosts: settings.mcp_allowed_hosts.clone(),
            create_store: Arc::new(create_store),
            store_administration: Arc::new(store_administration),
            inventory_management: Arc::new(inventory_management),
            create_product: Arc::new(create_product),
            catalog_queries: Arc::new(catalog_queries),
            catalog_management: Arc::new(catalog_management),
            collection_administration: Arc::new(collection_administration),
            storefront_collections: Arc::new(storefront_collections),
            review_administration: Arc::new(review_administration),
            storefront_reviews: Arc::new(storefront_reviews),
            media_administration: Arc::new(media_administration),
            create_price_list: Arc::new(create_price_list),
            pricing_management: Arc::new(pricing_management),
            store_queries: Arc::new(store_queries),
            store_membership_management: Arc::new(store_membership_management),
            publishable_key_management: Arc::new(publishable_key_management),
            publishable_key_authentication: Arc::new(publishable_key_authentication),
            provider_secret_management: Arc::new(provider_secret_management),
            analytics_collection: Arc::new(analytics_collection),
            analytics_administration: Arc::new(analytics_administration),
            storefront_catalog: Arc::new(storefront_catalog),
            storefront_sales: Arc::new(storefront_sales),
            order_management: Arc::new(order_management),
            payment_service: Arc::new(payment_service),
            stripe_account_administration: Arc::new(stripe_account_administration),
            clock: Arc::new(SystemClock),
            shopper_credentials: Arc::new(shopper_credentials),
        })
    }
}

pub fn router(state: ApiState) -> Router {
    let mcp_router = crate::mcp::router(state.clone());
    Router::new()
        .nest("/health", health::routes())
        .nest("/identity/v1", identity::v1::routes())
        .nest("/storefront/v1", storefront::v1::routes())
        .with_state(state)
        .nest("/mcp/v1", mcp_router)
        .layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode},
    };
    use chaos_infrastructure::runtime::{config::Settings, state::AppState};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    pub(crate) fn test_state() -> ApiState {
        let settings = Settings {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: "postgres://localhost/chaos".into(),
            database_identity_url: "postgres://localhost/chaos".into(),
            database_max_connections: 1,
            database_identity_max_connections: 1,
            database_analytics_max_connections: 1,
            database_analytics_statement_timeout: Duration::from_millis(10),
            database_acquire_timeout: Duration::from_millis(10),
            database_runtime_role: None,
            database_identity_role: None,
            redis_url: "redis://localhost".into(),
            auth_jwt_issuer: "https://identity.chaos.test".into(),
            auth_jwt_audience: "chaos-api".into(),
            auth_jwt_secret: secrecy::SecretString::from(
                "test-jwt-secret-that-is-at-least-32-bytes",
            ),
            auth_jwt_lifetime_seconds: 3600,
            mcp_allowed_hosts: vec!["localhost".into()],
            public_base_url: "http://localhost:8080/".parse().unwrap(),
            google_client_id: Some("test-google-client".into()),
            apple_client_id: None,
            storefront_public_base_url: "http://localhost:4321/".parse().unwrap(),
            stripe_api_base_url: "http://127.0.0.1:12111/".parse().unwrap(),
            easypost_api_base_url: "http://127.0.0.1:12113/".parse().unwrap(),
            analytics_meta_api_base_url: "http://127.0.0.1:12114/".parse().unwrap(),
            provider_secret_key: chaos_infrastructure::runtime::config::SecretKey::from_base64(
                "MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=",
            )
            .unwrap(),
            media_storage: None,
            shopper_token_active_key_id: "test".into(),
            shopper_token_active_secret: "test-shopper-token-secret-32-bytes".into(),
            shopper_token_previous_key: None,
            dependency_timeout: Duration::from_millis(10),
            shutdown_drain_delay: Duration::ZERO,
            shutdown_worker_timeout: Duration::from_secs(1),
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
    async fn liveness_uses_the_success_envelope_without_synthetic_request_ids() {
        let response = router(test_state())
            .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-request-id").is_none());

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["data"]["status"],
            "ok"
        );
    }

    #[tokio::test]
    async fn route_registry_contains_each_public_boundary() {
        let state = test_state();
        let requests = [
            (Method::GET, "/health/live"),
            (Method::POST, "/identity/v1/auth/external"),
            (Method::GET, "/storefront/v1/products"),
            (Method::GET, "/storefront/v1/collections"),
            (Method::POST, "/storefront/v1/analytics/events"),
            (Method::POST, "/storefront/v1/carts"),
            (
                Method::POST,
                "/storefront/v1/webhooks/stripe/00000000-0000-0000-0000-000000000000",
            ),
        ];

        for (method, path) in requests {
            let response = router(state.clone())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "route missing: {path}"
            );
        }
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
                Request::post("/identity/v1/auth/external")
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
    async fn storefront_catalog_rejects_requests_without_a_machine_credential() {
        let response = router(test_state())
            .oneshot(
                Request::get("/storefront/v1/products")
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
    async fn legacy_admin_http_surface_is_not_routed() {
        let response = router(test_state())
            .oneshot(
                Request::get("/admin/v1/stores")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn mcp_transport_is_stateless_and_rejects_unconfigured_hosts() {
        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "chaos-test", "version": "1.0.0" }
            }
        });
        let request = |host: &'static str| {
            Request::post("/mcp/v1")
                .header("host", host)
                .header("accept", "application/json, text/event-stream")
                .header("content-type", "application/json")
                .body(Body::from(initialize.to_string()))
                .unwrap()
        };

        let app = router(test_state());
        let rejected = app
            .clone()
            .oneshot(request("untrusted.example"))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

        let accepted = app.oneshot(request("localhost")).await.unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
        assert!(accepted.headers().get("mcp-session-id").is_none());
    }
}
