#[path = "storefront/analytics.rs"]
mod analytics;
#[path = "identity/auth.rs"]
mod auth;
#[path = "storefront/collections.rs"]
mod collection;
#[path = "storefront/customers.rs"]
mod customer;
#[path = "shared/error.rs"]
mod error;
#[path = "shared/extract.rs"]
mod extract;
#[path = "operations/health.rs"]
mod health;
#[path = "operations/metrics.rs"]
mod metrics;
#[path = "webhooks/notification.rs"]
mod notification;
#[path = "shared/openapi.rs"]
mod openapi;
#[path = "shared/pagination.rs"]
mod pagination;
#[path = "storefront/payments.rs"]
mod payment;
#[path = "shared/response.rs"]
mod response;
#[path = "storefront/reviews.rs"]
mod review;
#[path = "storefront/catalog.rs"]
mod storefront;
#[path = "storefront/sales.rs"]
mod storefront_sales;
#[cfg(test)]
#[path = "shared/test_support.rs"]
mod test_support;

use axum::Router;
use chaos_application::{
    analytics::{AnalyticsAdministration, AnalyticsCollection, AnalyticsPrivacy},
    catalog::{
        CatalogLocalization, CatalogManagement, CatalogQueries, CollectionAdministration,
        CreateProduct, MediaAdministration, ReviewAdministration, StorefrontCollections,
        StorefrontReviews,
    },
    fulfillment::{FulfillmentManagement, ShippingManagement, ShippingProviderAdministration},
    identity::{AccessKeyAuthentication, AccessKeyManagement, IdentityService},
    inventory::InventoryManagement,
    notifications::{NotificationProviderAdministration, NotificationWebhooks},
    payments::{PaymentProviderAdministration, PaymentService},
    ports::{Clock, IdentityAuthentication, MediaStorage, ShopperCredentialCodec},
    pricing::{CreatePriceList, PricingManagement, PromotionManagement, TaxManagement},
    sales::{CustomerService, OrderManagement, StorefrontSales},
    store::{
        CreateStore, ProviderSecretManagement, PublishableKeyAuthentication,
        PublishableKeyManagement, StoreAdministration, StoreMembershipManagement, StoreQueries,
    },
    storefront::StorefrontCatalog,
};
use std::sync::Arc;

use chaos_infrastructure::{
    RedisAnalyticsCollectionRateLimiter,
    clock::SystemClock,
    config::Settings,
    easypost::EasyPostShippingProvider,
    email::ResendWebhookVerifier,
    identity::{
        JwtAccessTokenCodec, OidcIdentityVerifier, OidcProviderConfiguration,
        PostgresAccessKeyRepository, PostgresIdentityRepository, SecureAccessKeyMaterialGenerator,
    },
    media_storage::{S3MediaStorage, S3MediaStorageConfiguration, UnavailableMediaStorage},
    repositories::{
        HmacPaymentWebhookVerifier, PostgresAnalyticsEventRepository,
        PostgresCatalogLocalizationRepository, PostgresCatalogManagementUnitOfWork,
        PostgresCatalogProvisioningUnitOfWork, PostgresCatalogReadRepository,
        PostgresCollectionRepository, PostgresCustomerRepository, PostgresEmailDeliveryRepository,
        PostgresFulfillmentRepository, PostgresInventoryRepository, PostgresMediaAssetRepository,
        PostgresOrderManagementRepository, PostgresPaymentRepository,
        PostgresPricingManagementRepository, PostgresPricingProvisioningUnitOfWork,
        PostgresPromotionRepository, PostgresPublishableKeyRepository, PostgresReviewRepository,
        PostgresShippingServiceRepository, PostgresStoreAdministrationRepository,
        PostgresStoreMembershipRepository, PostgresStoreProvisioningUnitOfWork,
        PostgresStoreReadRepository, PostgresStorefrontCatalogRepository,
        PostgresStorefrontSalesRepository, PostgresTaxRuleRepository, SandboxPaymentProvider,
        SecurePublishableKeyMaterialGenerator,
    },
    secret::DynamicSecretResolver,
    shopper::HmacShopperCredentialCodec,
    state::AppState,
    stripe::{StripeCheckoutPaymentProvider, StripePaymentProvider, StripeWebhookVerifier},
};
use metrics_exporter_prometheus::PrometheusHandle;
use secrecy::ExposeSecret as _;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::lifecycle::Lifecycle;

pub use error::{ApiError, ErrorBody, ErrorDetail, ErrorEnvelope};
pub use extract::{
    AnalyticsCustomer, AnalyticsMachine, ApiJson, ApiPath, ApiQuery, AuthenticatedUser,
    CartMachine, CartShopper, CheckoutShopper, CustomerCheckout, CustomerMachine, CustomerSession,
    OrderLookupMachine, StoreContext, StorefrontMachine,
};
pub use response::{ApiDateTime, ApiResponse, PageMeta, ResponseEnvelope, ResponseMeta};

#[derive(Clone)]
pub struct ApiState {
    pub infrastructure: AppState,
    pub lifecycle: Lifecycle,
    pub metrics: PrometheusHandle,
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
    pub catalog_localization: Arc<CatalogLocalization>,
    pub create_price_list: Arc<CreatePriceList>,
    pub pricing_management: Arc<PricingManagement>,
    pub tax_management: Arc<TaxManagement>,
    pub promotion_management: Arc<PromotionManagement>,
    pub store_queries: Arc<StoreQueries>,
    pub store_membership_management: Arc<StoreMembershipManagement>,
    pub publishable_key_management: Arc<PublishableKeyManagement>,
    pub publishable_key_authentication: Arc<PublishableKeyAuthentication>,
    pub provider_secret_management: Arc<ProviderSecretManagement>,
    pub analytics_collection: Arc<AnalyticsCollection>,
    pub analytics_administration: Arc<AnalyticsAdministration>,
    pub analytics_privacy: Arc<AnalyticsPrivacy>,
    pub storefront_catalog: Arc<StorefrontCatalog>,
    pub storefront_sales: Arc<StorefrontSales>,
    pub customer_service: Arc<CustomerService>,
    pub order_management: Arc<OrderManagement>,
    pub payment_service: Arc<PaymentService>,
    pub payment_provider_administration: Arc<PaymentProviderAdministration>,
    pub notification_webhooks: Arc<NotificationWebhooks>,
    pub notification_provider_administration: Arc<NotificationProviderAdministration>,
    pub fulfillment_management: Arc<FulfillmentManagement>,
    pub shipping_management: Arc<ShippingManagement>,
    pub shipping_provider_administration: Arc<ShippingProviderAdministration>,
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
        let catalog_localization = CatalogLocalization::new(Arc::new(
            PostgresCatalogLocalizationRepository::new(infrastructure.runtime_pool()),
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
        let tax_management = TaxManagement::new(Arc::new(PostgresTaxRuleRepository::new(
            infrastructure.runtime_pool(),
        )));
        let promotion_management = PromotionManagement::new(Arc::new(
            PostgresPromotionRepository::new(infrastructure.runtime_pool()),
        ));
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
            Arc::new(SecurePublishableKeyMaterialGenerator),
        );
        let publishable_key_authentication =
            PublishableKeyAuthentication::new(publishable_key_repository);
        let analytics_repository = Arc::new(PostgresAnalyticsEventRepository::new(
            infrastructure.runtime_pool(),
        ));
        let analytics_collection = AnalyticsCollection::new(
            analytics_repository.clone(),
            Arc::new(RedisAnalyticsCollectionRateLimiter::new(
                infrastructure.redis_client(),
            )),
        );
        let analytics_administration = AnalyticsAdministration::new(
            analytics_repository.clone(),
            analytics_repository.clone(),
        );
        let analytics_privacy = AnalyticsPrivacy::new(analytics_repository);
        let dynamic_secrets = Arc::new(DynamicSecretResolver::new(&settings.provider_secret_key));
        let provider_secret_management =
            ProviderSecretManagement::new(store_administration_repository, dynamic_secrets.clone());
        let storefront_catalog = StorefrontCatalog::new(Arc::new(
            PostgresStorefrontCatalogRepository::new(infrastructure.runtime_pool()),
        ));
        let storefront_sales = StorefrontSales::new(Arc::new(
            PostgresStorefrontSalesRepository::new(infrastructure.runtime_pool()),
        ));
        let customer_service = CustomerService::new(Arc::new(PostgresCustomerRepository::new(
            infrastructure.runtime_pool(),
            infrastructure.identity_pool(),
        )));
        let order_management = OrderManagement::new(Arc::new(
            PostgresOrderManagementRepository::new(infrastructure.runtime_pool()),
        ));
        let payment_repository = Arc::new(PostgresPaymentRepository::new(
            infrastructure.runtime_pool(),
        ));
        let payment_secrets = dynamic_secrets.clone();
        let sandbox_payment_provider = Arc::new(SandboxPaymentProvider);
        let stripe_payment_provider = Arc::new(StripePaymentProvider::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            payment_secrets.clone(),
        )?);
        let stripe_checkout_payment_provider = Arc::new(StripeCheckoutPaymentProvider::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            payment_secrets.clone(),
        )?);
        let providers = vec![
            sandbox_payment_provider.clone() as Arc<dyn chaos_application::ports::PaymentProvider>,
            stripe_payment_provider.clone() as Arc<dyn chaos_application::ports::PaymentProvider>,
            stripe_checkout_payment_provider.clone()
                as Arc<dyn chaos_application::ports::PaymentProvider>,
        ];
        let payment_onboarding = vec![
            sandbox_payment_provider
                as Arc<dyn chaos_application::ports::PaymentProviderOnboarding>,
            stripe_payment_provider as Arc<dyn chaos_application::ports::PaymentProviderOnboarding>,
            stripe_checkout_payment_provider
                as Arc<dyn chaos_application::ports::PaymentProviderOnboarding>,
        ];
        let webhook_verifiers = vec![
            Arc::new(HmacPaymentWebhookVerifier::new(
                settings.payment_webhook_secret.as_bytes(),
            )?) as Arc<dyn chaos_application::ports::PaymentWebhookVerifier>,
            Arc::new(StripeWebhookVerifier::new(
                payment_repository.clone(),
                payment_secrets.clone(),
            )) as Arc<dyn chaos_application::ports::PaymentWebhookVerifier>,
            Arc::new(StripeWebhookVerifier::for_provider(
                "stripe_checkout",
                payment_repository.clone(),
                payment_secrets,
            )) as Arc<dyn chaos_application::ports::PaymentWebhookVerifier>,
        ];
        let payment_service = PaymentService::new(
            payment_repository.clone(),
            webhook_verifiers,
            providers.clone(),
        );
        let payment_provider_administration = PaymentProviderAdministration::new(
            payment_repository.clone(),
            payment_onboarding.clone(),
        );
        let notification_repository = Arc::new(PostgresEmailDeliveryRepository::new(
            infrastructure.runtime_pool(),
        ));
        let notification_verifier = Arc::new(ResendWebhookVerifier::new(dynamic_secrets.clone()))
            as Arc<dyn chaos_application::ports::EmailWebhookVerifier>;
        let notification_webhooks = NotificationWebhooks::new(
            notification_repository.clone(),
            notification_repository.clone(),
            [notification_verifier],
        );
        let notification_provider_administration =
            NotificationProviderAdministration::new(notification_repository);
        let fulfillment_repository = Arc::new(PostgresFulfillmentRepository::new(
            infrastructure.runtime_pool(),
        ));
        let fulfillment_management = FulfillmentManagement::new(fulfillment_repository);
        let shipping_repository = Arc::new(PostgresShippingServiceRepository::new(
            infrastructure.runtime_pool(),
        ));
        let shipping_management = ShippingManagement::new(shipping_repository.clone());
        let shipping_provider: Arc<dyn chaos_application::ports::ShippingProvider> =
            Arc::new(EasyPostShippingProvider::new(
                settings.easypost_api_base_url.clone(),
                settings.dependency_timeout,
                dynamic_secrets,
            )?);
        let shipping_provider_administration = ShippingProviderAdministration::new(
            shipping_repository.clone(),
            shipping_repository,
            [shipping_provider],
        );
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
            catalog_localization: Arc::new(catalog_localization),
            create_price_list: Arc::new(create_price_list),
            pricing_management: Arc::new(pricing_management),
            tax_management: Arc::new(tax_management),
            promotion_management: Arc::new(promotion_management),
            store_queries: Arc::new(store_queries),
            store_membership_management: Arc::new(store_membership_management),
            publishable_key_management: Arc::new(publishable_key_management),
            publishable_key_authentication: Arc::new(publishable_key_authentication),
            provider_secret_management: Arc::new(provider_secret_management),
            analytics_collection: Arc::new(analytics_collection),
            analytics_administration: Arc::new(analytics_administration),
            analytics_privacy: Arc::new(analytics_privacy),
            storefront_catalog: Arc::new(storefront_catalog),
            storefront_sales: Arc::new(storefront_sales),
            customer_service: Arc::new(customer_service),
            order_management: Arc::new(order_management),
            payment_service: Arc::new(payment_service),
            payment_provider_administration: Arc::new(payment_provider_administration),
            notification_webhooks: Arc::new(notification_webhooks),
            notification_provider_administration: Arc::new(notification_provider_administration),
            fulfillment_management: Arc::new(fulfillment_management),
            shipping_management: Arc::new(shipping_management),
            shipping_provider_administration: Arc::new(shipping_provider_administration),
            clock: Arc::new(SystemClock),
            shopper_credentials: Arc::new(shopper_credentials),
        })
    }
}

pub fn router(state: ApiState) -> Router {
    let mcp_router = chaos_mcp::router(
        chaos_mcp::McpState {
            access_key_authentication: state.access_key_authentication.clone(),
            store_queries: state.store_queries.clone(),
            store_membership_management: state.store_membership_management.clone(),
            create_store: state.create_store.clone(),
            catalog_queries: state.catalog_queries.clone(),
            create_product: state.create_product.clone(),
            catalog_management: state.catalog_management.clone(),
            collection_administration: state.collection_administration.clone(),
            pricing_management: state.pricing_management.clone(),
            create_price_list: state.create_price_list.clone(),
            promotion_management: state.promotion_management.clone(),
            tax_management: state.tax_management.clone(),
            inventory_management: state.inventory_management.clone(),
            order_management: state.order_management.clone(),
            fulfillment_management: state.fulfillment_management.clone(),
            shipping_management: state.shipping_management.clone(),
            shipping_provider_administration: state.shipping_provider_administration.clone(),
            store_administration: state.store_administration.clone(),
            payment_service: state.payment_service.clone(),
            payment_provider_administration: state.payment_provider_administration.clone(),
            notification_provider_administration: state
                .notification_provider_administration
                .clone(),
            media_administration: state.media_administration.clone(),
            catalog_localization: state.catalog_localization.clone(),
            review_administration: state.review_administration.clone(),
            publishable_key_management: state.publishable_key_management.clone(),
            provider_secret_management: state.provider_secret_management.clone(),
            analytics_administration: state.analytics_administration.clone(),
            analytics_privacy: state.analytics_privacy.clone(),
            clock: state.clock.clone(),
        },
        state.mcp_allowed_hosts.clone(),
    );
    Router::new()
        .nest("/health", health::routes())
        .nest("/metrics", metrics::routes())
        .nest("/identity/v1", auth::routes())
        .merge(payment::routes())
        .merge(notification::routes())
        .nest("/store/v1", storefront::routes())
        .nest("/store/v1", collection::storefront_routes())
        .nest("/store/v1", review::storefront_routes())
        .nest("/store/v1", analytics::storefront_routes())
        .nest("/store/v1", storefront_sales::routes())
        .nest("/store/v1", customer::routes())
        .nest("/openapi", openapi::routes())
        .with_state(state)
        .nest("/mcp/v1", mcp_router)
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
            google_client_id: Some("test-google-client".into()),
            apple_client_id: None,
            storefront_public_base_url: "http://localhost:4321/".parse().unwrap(),
            resend_api_base_url: "http://localhost:12112/".parse().unwrap(),
            payment_webhook_secret: "test-payment-webhook-secret-32-bytes".into(),
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

        // The read limit here is a generous sanity ceiling, not a tight
        // assertion on payload size: chaos_http_request_duration_seconds is a
        // 10-bucket histogram, and the process-wide Prometheus registry
        // (telemetry::init_metrics, a OnceLock) accumulates one label series
        // per distinct (method, route, status) triple exercised by every test
        // in this binary, not just this one — so the body only grows as the
        // test suite does and must not be bounded to a size tied to today's
        // route/test count.
        let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
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
    async fn identity_openapi_contract_is_publicly_available() {
        let response = router(test_state())
            .oneshot(
                Request::get("/openapi/identity-v1.json")
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

        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let contract = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(contract["info"]["title"], "Chaos Identity API");
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
