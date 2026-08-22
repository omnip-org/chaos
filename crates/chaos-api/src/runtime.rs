//! Runtime-specific dependency composition.

use std::sync::Arc;

use chaos_application::{
    analytics::AnalyticsDeliveryWorker,
    fulfillment::FulfillmentWorkers,
    payments::PaymentWorkers,
    ports::{
        AnalyticsEventDestination, Clock, PaymentProvider, PaymentProviderOnboarding,
        ShippingProvider,
    },
    sales::CheckoutExpiryWorkers,
};
use chaos_infrastructure::{
    clock::SystemClock,
    config::Settings,
    meta::MetaConversionsDestination,
    providers::{easypost::EasyPostShippingProvider, stripe::StripeCheckoutPaymentProvider},
    repositories::{
        PostgresAnalyticsEventRepository, PostgresFulfillmentRepository, PostgresPaymentRepository,
        PostgresSearchIndexer, PostgresShippingServiceRepository,
        PostgresStorefrontSalesRepository,
    },
    secret::DynamicSecretResolver,
    state::AppState,
};

/// Dependencies used by durable polling loops, without HTTP or MCP state.
#[derive(Clone)]
pub struct WorkerRuntime {
    pub infrastructure: AppState,
    pub payment_workers: Arc<PaymentWorkers>,
    pub fulfillment_workers: Arc<FulfillmentWorkers>,
    pub analytics_delivery_worker: Arc<AnalyticsDeliveryWorker>,
    pub search_indexer: Arc<PostgresSearchIndexer>,
    pub checkout_expiry_workers: Arc<CheckoutExpiryWorkers>,
    pub clock: Arc<dyn Clock>,
}

impl WorkerRuntime {
    pub fn new(infrastructure: &AppState, settings: &Settings) -> anyhow::Result<Self> {
        let dynamic_secrets = Arc::new(DynamicSecretResolver::new(&settings.provider_secret_key));

        let analytics_repository = Arc::new(PostgresAnalyticsEventRepository::new(
            infrastructure.runtime_pool(),
        ));
        let meta_destination = Arc::new(MetaConversionsDestination::new(
            settings.analytics_meta_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let analytics_delivery_worker = Arc::new(AnalyticsDeliveryWorker::new(
            analytics_repository,
            [meta_destination as Arc<dyn AnalyticsEventDestination>],
        ));

        let payment_repository = Arc::new(PostgresPaymentRepository::new(
            infrastructure.runtime_pool(),
        ));
        let stripe_checkout_payment_provider = Arc::new(StripeCheckoutPaymentProvider::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let payment_providers =
            vec![stripe_checkout_payment_provider.clone() as Arc<dyn PaymentProvider>];
        let payment_onboarding =
            vec![stripe_checkout_payment_provider as Arc<dyn PaymentProviderOnboarding>];
        let payment_workers = PaymentWorkers::new(
            payment_repository.clone(),
            payment_repository.clone(),
            payment_repository,
            payment_providers,
            payment_onboarding,
        );

        let fulfillment_repository = Arc::new(PostgresFulfillmentRepository::new(
            infrastructure.runtime_pool(),
        ));
        let shipping_repository = Arc::new(PostgresShippingServiceRepository::new(
            infrastructure.runtime_pool(),
        ));
        let shipping_provider: Arc<dyn ShippingProvider> = Arc::new(EasyPostShippingProvider::new(
            settings.easypost_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets,
        )?);
        let fulfillment_workers = FulfillmentWorkers::new(
            fulfillment_repository,
            shipping_repository,
            [shipping_provider],
        );

        let storefront_sales_repository = Arc::new(PostgresStorefrontSalesRepository::new(
            infrastructure.runtime_pool(),
        ));

        Ok(Self {
            infrastructure: infrastructure.clone(),
            payment_workers: Arc::new(payment_workers),
            fulfillment_workers: Arc::new(fulfillment_workers),
            analytics_delivery_worker,
            search_indexer: Arc::new(PostgresSearchIndexer::new(infrastructure.runtime_pool())),
            checkout_expiry_workers: Arc::new(CheckoutExpiryWorkers::new(
                storefront_sales_repository,
            )),
            clock: Arc::new(SystemClock),
        })
    }
}
