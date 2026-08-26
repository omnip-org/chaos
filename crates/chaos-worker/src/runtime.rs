//! Runtime-specific dependency composition.

use std::sync::Arc;

use chaos_core::{
    adapters::integrations::{
        analytics::meta::MetaConversionsDestination, manual_shipping::ManualShippingProvider,
        resend::ResendEmailProvider, stripe::StripeGateway,
    },
    adapters::postgres::{
        PostgresAnalyticsDeliveryStore, PostgresEmailRepository, PostgresIntegrationQueue,
        PostgresMaintenance, PostgresSearchIndexer, PostgresShippingRepository,
        PostgresStripeRepository,
    },
    adapters::security::provider_secrets::DynamicSecretResolver,
    runtime::{clock::SystemClock, config::Settings, state::AppState},
};
use chaos_core::{
    analytics::AnalyticsDeliveryWorker,
    contracts::{
        AnalyticsEventDestination, Clock, EmailProvider, IntegrationQueue, PaymentProvider,
        PaymentProviderRegistry,
    },
    email::EmailWorkers,
    payments::PaymentWorkers,
    shipping::ShippingWorkers,
};

/// Dependencies used by durable polling loops, without HTTP or MCP state.
#[derive(Clone)]
pub struct WorkerRuntime {
    pub payment_workers: Arc<PaymentWorkers>,
    pub email_workers: Arc<EmailWorkers>,
    pub shipping_workers: Arc<ShippingWorkers>,
    pub analytics_delivery_worker: Arc<AnalyticsDeliveryWorker>,
    pub search_indexer: Arc<PostgresSearchIndexer>,
    pub maintenance: Arc<PostgresMaintenance>,
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

        let payment_repository =
            Arc::new(PostgresStripeRepository::new(infrastructure.runtime_pool()));
        let integration_queue: Arc<dyn IntegrationQueue> =
            Arc::new(PostgresIntegrationQueue::new(infrastructure.runtime_pool()));
        let stripe_gateway = Arc::new(StripeGateway::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let payment_providers = Arc::new(PaymentProviderRegistry::new([
            stripe_gateway as Arc<dyn PaymentProvider>
        ]));
        let payment_workers = PaymentWorkers::new(
            integration_queue.clone(),
            payment_repository,
            payment_providers,
        );
        let email_provider = Arc::new(ResendEmailProvider::new(
            settings.resend_api_base_url.clone(),
            dynamic_secrets.clone(),
            settings.dependency_timeout,
        )?) as Arc<dyn EmailProvider>;
        let shipping_queue = integration_queue.clone();
        let email_workers = EmailWorkers::new(
            integration_queue,
            Arc::new(PostgresEmailRepository::new(infrastructure.runtime_pool())),
            [email_provider],
        );
        let shipping_workers = ShippingWorkers::new(
            shipping_queue,
            Arc::new(PostgresShippingRepository::new(
                infrastructure.runtime_pool(),
            )),
            [Arc::new(ManualShippingProvider)
                as Arc<dyn chaos_core::contracts::ShippingProvider>],
        );

        Ok(Self {
            payment_workers: Arc::new(payment_workers),
            email_workers: Arc::new(email_workers),
            shipping_workers: Arc::new(shipping_workers),
            analytics_delivery_worker,
            search_indexer: Arc::new(PostgresSearchIndexer::new(infrastructure.runtime_pool())),
            maintenance: Arc::new(PostgresMaintenance::new(
                infrastructure.runtime_pool(),
                infrastructure.identity_pool(),
            )),
            clock: Arc::new(SystemClock),
        })
    }
}
