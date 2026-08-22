//! Runtime-specific dependency composition.

use std::sync::Arc;

use chaos_application::{
    analytics::AnalyticsDeliveryWorker,
    fulfillment::FulfillmentWorkers,
    payments::PaymentWorkers,
    ports::{
        AnalyticsEventDestination, Clock, IntegrationQueue, ShippingProvider,
        StripeAccountReadiness, StripePaymentGateway,
    },
};
use chaos_infrastructure::{
    integrations::{
        analytics::meta::MetaConversionsDestination, payments::stripe::StripeGateway,
        shipping::easypost::EasyPostShippingProvider,
    },
    repositories::{
        PostgresAnalyticsDeliveryStore, PostgresFulfillmentRepository, PostgresIntegrationQueue,
        PostgresPaymentRepository, PostgresSearchIndexer, PostgresShippingServiceRepository,
    },
    runtime::{clock::SystemClock, config::Settings, state::AppState},
    security::provider_secrets::DynamicSecretResolver,
};

/// Dependencies used by durable polling loops, without HTTP or MCP state.
#[derive(Clone)]
pub struct WorkerRuntime {
    pub infrastructure: AppState,
    pub payment_workers: Arc<PaymentWorkers>,
    pub fulfillment_workers: Arc<FulfillmentWorkers>,
    pub analytics_delivery_worker: Arc<AnalyticsDeliveryWorker>,
    pub search_indexer: Arc<PostgresSearchIndexer>,
    pub clock: Arc<dyn Clock>,
}

impl WorkerRuntime {
    pub fn new(infrastructure: &AppState, settings: &Settings) -> anyhow::Result<Self> {
        let dynamic_secrets = Arc::new(DynamicSecretResolver::new(&settings.provider_secret_key));

        let analytics_delivery_store = Arc::new(PostgresAnalyticsDeliveryStore::new(
            infrastructure.runtime_pool(),
        ));
        let meta_destination = Arc::new(MetaConversionsDestination::new(
            settings.analytics_meta_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let analytics_delivery_worker = Arc::new(AnalyticsDeliveryWorker::new(
            analytics_delivery_store,
            [meta_destination as Arc<dyn AnalyticsEventDestination>],
        ));

        let payment_repository = Arc::new(PostgresPaymentRepository::new(
            infrastructure.runtime_pool(),
        ));
        let integration_queue: Arc<dyn IntegrationQueue> =
            Arc::new(PostgresIntegrationQueue::new(infrastructure.runtime_pool()));
        let stripe_gateway = Arc::new(StripeGateway::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let payment_provider = stripe_gateway.clone() as Arc<dyn StripePaymentGateway>;
        let payment_onboarding = stripe_gateway as Arc<dyn StripeAccountReadiness>;
        let payment_workers = PaymentWorkers::new(
            integration_queue,
            payment_repository.clone(),
            payment_repository,
            payment_provider,
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

        Ok(Self {
            infrastructure: infrastructure.clone(),
            payment_workers: Arc::new(payment_workers),
            fulfillment_workers: Arc::new(fulfillment_workers),
            analytics_delivery_worker,
            search_indexer: Arc::new(PostgresSearchIndexer::new(infrastructure.runtime_pool())),
            clock: Arc::new(SystemClock),
        })
    }
}
