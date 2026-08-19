mod analytics;
mod api_key;
mod auth;
mod catalog;
mod collection;
mod customer;
mod error;
mod extract;
mod fulfillment;
mod health;
mod inventory;
mod localization;
mod media;
mod merchant;
mod metrics;
mod notification;
mod openapi;
mod order;
mod payment;
mod pricing;
mod provider_secret;
mod response;
mod review;
mod store_admin;
mod storefront;
mod storefront_sales;

use anyhow::Context as _;
use axum::Router;
use chaos_application::{
    analytics::{
        AnalyticsAdministration, AnalyticsCollection, AnalyticsDestinations, AnalyticsPrivacy,
        AnalyticsReporting, AnalyticsWorkers,
    },
    catalog::{
        CatalogLocalization, CatalogManagement, CatalogQueries, CollectionAdministration,
        CreateProduct, MediaAdministration, ReviewAdministration, StorefrontCollections,
        StorefrontReviews,
    },
    fulfillment::{
        FulfillmentManagement, FulfillmentWorkers, ShippingManagement,
        ShippingProviderAdministration,
    },
    inventory::InventoryManagement,
    merchant::{
        ApiKeyAuthentication, ApiKeyManagement, CreateStore, MerchantQueries,
        ProviderSecretManagement, StoreAdministration,
    },
    notifications::{NotificationWebhooks, NotificationWorkers},
    payments::{PaymentProviderAdministration, PaymentService, PaymentWorkers},
    ports::{Clock, MediaStorage, PasswordlessAuthentication, ShopperCredentialCodec},
    pricing::{CreatePriceList, PricingManagement, PromotionManagement, TaxManagement},
    sales::{CheckoutExpiryWorkers, CustomerService, OrderManagement, StorefrontSales},
    storefront::StorefrontCatalog,
};
use std::sync::Arc;

use chaos_infrastructure::{
    RedisAnalyticsCollectionRateLimiter,
    analytics_destinations::{Ga4MeasurementDestination, MetaConversionsDestination},
    clock::SystemClock,
    config::Settings,
    easypost::EasyPostShippingProvider,
    email::{ResendEmailProvider, ResendWebhookVerifier, SmtpEmailProvider},
    media_storage::{S3MediaStorage, S3MediaStorageConfiguration, UnavailableMediaStorage},
    passwordless::PasswordlessAuth,
    repositories::{
        HmacPaymentWebhookVerifier, PostgresAnalyticsEventRepository,
        PostgresAnalyticsReportingRepository, PostgresApiKeyRepository,
        PostgresCatalogLocalizationRepository, PostgresCatalogManagementUnitOfWork,
        PostgresCatalogProvisioningUnitOfWork, PostgresCatalogReadRepository,
        PostgresCollectionRepository, PostgresCustomerRepository, PostgresEmailDeliveryRepository,
        PostgresFulfillmentRepository, PostgresInventoryRepository, PostgresMediaAssetRepository,
        PostgresMerchantReadRepository, PostgresOrderManagementRepository,
        PostgresPaymentRepository, PostgresPricingManagementRepository,
        PostgresPricingProvisioningUnitOfWork, PostgresPromotionRepository,
        PostgresReviewRepository, PostgresSearchIndexer, PostgresShippingServiceRepository,
        PostgresStoreAdministrationRepository, PostgresStoreProvisioningUnitOfWork,
        PostgresStorefrontCatalogRepository, PostgresStorefrontSalesRepository,
        PostgresTaxRuleRepository, SandboxPaymentProvider, SecureApiKeyMaterialGenerator,
    },
    secret::DynamicSecretResolver,
    shopper::HmacShopperCredentialCodec,
    state::AppState,
    stripe::{StripeCheckoutPaymentProvider, StripePaymentProvider, StripeWebhookVerifier},
};
use metrics_exporter_prometheus::PrometheusHandle;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::lifecycle::Lifecycle;

pub use error::{ApiError, ErrorBody, ErrorDetail, ErrorEnvelope};
pub use extract::{
    AnalyticsCustomer, AnalyticsMachine, ApiJson, ApiPath, ApiQuery, AuthenticatedSession,
    CartMachine, CartShopper, CheckoutShopper, CustomerCheckout, CustomerMachine, CustomerSession,
    OrderLookupMachine, StoreContext, StorefrontMachine,
};
pub use response::{ApiDateTime, ApiResponse, PageMeta, ResponseEnvelope, ResponseMeta};

#[derive(Clone)]
pub struct ApiState {
    pub infrastructure: AppState,
    pub lifecycle: Lifecycle,
    pub metrics: PrometheusHandle,
    pub passwordless_auth: Arc<dyn PasswordlessAuthentication>,
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
    pub merchant_queries: Arc<MerchantQueries>,
    pub api_key_management: Arc<ApiKeyManagement>,
    pub api_key_authentication: Arc<ApiKeyAuthentication>,
    pub provider_secret_management: Arc<ProviderSecretManagement>,
    pub analytics_collection: Arc<AnalyticsCollection>,
    pub analytics_administration: Arc<AnalyticsAdministration>,
    pub analytics_privacy: Arc<AnalyticsPrivacy>,
    pub analytics_reporting: Arc<AnalyticsReporting>,
    pub analytics_destinations: Arc<AnalyticsDestinations>,
    pub analytics_workers: Arc<AnalyticsWorkers>,
    pub storefront_catalog: Arc<StorefrontCatalog>,
    pub storefront_sales: Arc<StorefrontSales>,
    pub customer_service: Arc<CustomerService>,
    pub checkout_expiry_workers: Arc<CheckoutExpiryWorkers>,
    pub order_management: Arc<OrderManagement>,
    pub payment_service: Arc<PaymentService>,
    pub payment_provider_administration: Arc<PaymentProviderAdministration>,
    pub payment_workers: Arc<PaymentWorkers>,
    pub notification_workers: Arc<NotificationWorkers>,
    pub notification_webhooks: Arc<NotificationWebhooks>,
    pub fulfillment_management: Arc<FulfillmentManagement>,
    pub fulfillment_workers: Arc<FulfillmentWorkers>,
    pub shipping_management: Arc<ShippingManagement>,
    pub shipping_provider_administration: Arc<ShippingProviderAdministration>,
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
        let email_provider: Arc<dyn chaos_application::ports::EmailProvider> =
            if let Some(api_key) = &settings.resend_api_key {
                Arc::new(ResendEmailProvider::new(
                    settings.resend_api_base_url.clone(),
                    api_key.clone(),
                    settings.dependency_timeout,
                )?)
            } else {
                let smtp_url = settings
                    .smtp_url
                    .as_deref()
                    .context("SMTP_URL must be set when RESEND_API_KEY is not")?;
                Arc::new(SmtpEmailProvider::new(smtp_url)?)
            };
        let passwordless_auth = PasswordlessAuth::new(
            infrastructure.control_plane_pool(),
            infrastructure.redis_client(),
            &settings.webauthn_rp_id,
            &settings.webauthn_rp_origin,
            email_provider.clone(),
            &settings.email_from,
            &settings.auth_public_base_url,
        )?;
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
        let analytics_repository = Arc::new(PostgresAnalyticsEventRepository::new(
            infrastructure.runtime_pool(),
        ));
        let analytics_collection = AnalyticsCollection::new(
            analytics_repository.clone(),
            Arc::new(RedisAnalyticsCollectionRateLimiter::new(
                infrastructure.redis_client(),
            )),
        );
        let analytics_administration = AnalyticsAdministration::new(analytics_repository.clone());
        let analytics_privacy = AnalyticsPrivacy::new(analytics_repository.clone());
        let analytics_reporting = AnalyticsReporting::new(Arc::new(
            PostgresAnalyticsReportingRepository::new(infrastructure.analytics_pool()),
        ));
        let analytics_destinations = AnalyticsDestinations::new(analytics_repository.clone());
        let dynamic_secrets = Arc::new(DynamicSecretResolver::new(&settings.provider_secret_key));
        let provider_secret_management =
            ProviderSecretManagement::new(store_administration_repository, dynamic_secrets.clone());
        let analytics_destination_secrets = dynamic_secrets.clone();
        let analytics_destination_adapters = vec![
            Arc::new(MetaConversionsDestination::new(
                settings.analytics_meta_api_base_url.clone(),
                settings.dependency_timeout,
                analytics_destination_secrets.clone(),
            )?) as Arc<dyn chaos_application::ports::AnalyticsDestination>,
            Arc::new(Ga4MeasurementDestination::new(
                settings.analytics_ga4_api_base_url.clone(),
                settings.dependency_timeout,
                analytics_destination_secrets,
            )?) as Arc<dyn chaos_application::ports::AnalyticsDestination>,
        ];
        let analytics_workers = AnalyticsWorkers::new(
            analytics_repository.clone(),
            analytics_repository.clone(),
            analytics_repository.clone(),
            analytics_repository.clone(),
            analytics_repository,
            analytics_destination_adapters,
        );
        let storefront_catalog = StorefrontCatalog::new(Arc::new(
            PostgresStorefrontCatalogRepository::new(infrastructure.runtime_pool()),
        ));
        let storefront_sales_repository = Arc::new(PostgresStorefrontSalesRepository::new(
            infrastructure.runtime_pool(),
        ));
        let storefront_sales = StorefrontSales::new(storefront_sales_repository.clone());
        let customer_service = CustomerService::new(Arc::new(PostgresCustomerRepository::new(
            infrastructure.runtime_pool(),
            infrastructure.control_plane_pool(),
        )));
        let checkout_expiry_workers = CheckoutExpiryWorkers::new(storefront_sales_repository);
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
        let payment_workers = PaymentWorkers::new(
            payment_repository.clone(),
            payment_repository.clone(),
            payment_repository,
            providers,
            payment_onboarding,
        );
        let notification_repository = Arc::new(PostgresEmailDeliveryRepository::new(
            infrastructure.runtime_pool(),
        ));
        let notification_providers = if email_provider.name() == "resend" {
            vec![email_provider]
        } else {
            Vec::new()
        };
        let notification_workers = NotificationWorkers::new(
            notification_repository.clone(),
            notification_providers,
            settings.email_from.clone(),
        );
        let notification_verifiers = settings
            .resend_webhook_secret
            .as_ref()
            .map(ResendWebhookVerifier::new)
            .transpose()?
            .into_iter()
            .map(|verifier| {
                Arc::new(verifier) as Arc<dyn chaos_application::ports::EmailWebhookVerifier>
            });
        let notification_webhooks =
            NotificationWebhooks::new(notification_repository, notification_verifiers);
        let fulfillment_repository = Arc::new(PostgresFulfillmentRepository::new(
            infrastructure.runtime_pool(),
        ));
        let fulfillment_management = FulfillmentManagement::new(fulfillment_repository.clone());
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
            shipping_repository.clone(),
            [shipping_provider.clone()],
        );
        let fulfillment_workers = FulfillmentWorkers::new(
            fulfillment_repository,
            shipping_repository,
            [shipping_provider],
        );
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
            merchant_queries: Arc::new(merchant_queries),
            api_key_management: Arc::new(api_key_management),
            api_key_authentication: Arc::new(api_key_authentication),
            provider_secret_management: Arc::new(provider_secret_management),
            analytics_collection: Arc::new(analytics_collection),
            analytics_administration: Arc::new(analytics_administration),
            analytics_privacy: Arc::new(analytics_privacy),
            analytics_reporting: Arc::new(analytics_reporting),
            analytics_destinations: Arc::new(analytics_destinations),
            analytics_workers: Arc::new(analytics_workers),
            storefront_catalog: Arc::new(storefront_catalog),
            storefront_sales: Arc::new(storefront_sales),
            customer_service: Arc::new(customer_service),
            checkout_expiry_workers: Arc::new(checkout_expiry_workers),
            order_management: Arc::new(order_management),
            payment_service: Arc::new(payment_service),
            payment_provider_administration: Arc::new(payment_provider_administration),
            payment_workers: Arc::new(payment_workers),
            notification_workers: Arc::new(notification_workers),
            notification_webhooks: Arc::new(notification_webhooks),
            fulfillment_management: Arc::new(fulfillment_management),
            fulfillment_workers: Arc::new(fulfillment_workers),
            shipping_management: Arc::new(shipping_management),
            shipping_provider_administration: Arc::new(shipping_provider_administration),
            search_indexer: Arc::new(search_indexer),
            clock: Arc::new(SystemClock),
            shopper_credentials: Arc::new(shopper_credentials),
        })
    }
}

pub fn router(state: ApiState) -> Router {
    let mcp_router = chaos_mcp::router(chaos_mcp::McpState {
        api_key_authentication: state.api_key_authentication.clone(),
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
        media_administration: state.media_administration.clone(),
        catalog_localization: state.catalog_localization.clone(),
        review_administration: state.review_administration.clone(),
        api_key_management: state.api_key_management.clone(),
        provider_secret_management: state.provider_secret_management.clone(),
        clock: state.clock.clone(),
    });
    Router::new()
        .nest("/health", health::routes())
        .nest("/metrics", metrics::routes())
        .nest("/admin/v1/auth", auth::routes())
        .nest("/admin/v1", merchant::routes())
        .nest("/admin/v1", store_admin::routes())
        .nest("/admin/v1", analytics::admin_routes())
        .nest("/admin/v1", inventory::routes())
        .nest("/admin/v1", order::routes())
        .nest("/admin/v1", fulfillment::routes())
        .merge(payment::routes())
        .merge(notification::routes())
        .nest("/admin/v1", catalog::routes())
        .nest("/admin/v1", collection::admin_routes())
        .nest("/admin/v1", review::admin_routes())
        .nest("/admin/v1", media::routes())
        .nest("/admin/v1", localization::routes())
        .nest("/admin/v1", pricing::routes())
        .nest("/admin/v1", api_key::routes())
        .nest("/admin/v1", provider_secret::routes())
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
            database_control_plane_url: "postgres://localhost/chaos".into(),
            database_max_connections: 1,
            database_control_plane_max_connections: 1,
            database_analytics_max_connections: 1,
            database_analytics_statement_timeout: Duration::from_millis(10),
            database_acquire_timeout: Duration::from_millis(10),
            database_runtime_role: None,
            database_control_plane_role: None,
            redis_url: "redis://localhost".into(),
            webauthn_rp_id: "localhost".into(),
            webauthn_rp_origin: "http://localhost:8080".into(),
            auth_public_base_url: "http://localhost:8080".into(),
            smtp_url: Some("smtp://localhost:1025".into()),
            email_from: "Chaos <no-reply@localhost>".into(),
            resend_api_key: None,
            resend_webhook_secret: None,
            resend_api_base_url: "http://localhost:12112/".parse().unwrap(),
            payment_webhook_secret: "test-payment-webhook-secret-32-bytes".into(),
            stripe_api_base_url: "http://127.0.0.1:12111/".parse().unwrap(),
            easypost_api_base_url: "http://127.0.0.1:12113/".parse().unwrap(),
            analytics_meta_api_base_url: "http://127.0.0.1:12114/".parse().unwrap(),
            analytics_ga4_api_base_url: "http://127.0.0.1:12115/".parse().unwrap(),
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

        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
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
        let store_id = uuid::Uuid::now_v7();
        let response = router(test_state())
            .oneshot(
                Request::get(format!("/admin/v1/stores/{store_id}/price-lists"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
