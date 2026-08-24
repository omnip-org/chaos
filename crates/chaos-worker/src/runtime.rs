//! Runtime-specific dependency composition.

use std::sync::Arc;

use chaos_core::{
    adapters::integrations::{analytics::meta::MetaConversionsDestination, stripe::StripeGateway},
    adapters::postgres::{
        PostgresAnalyticsDeliveryStore, PostgresIntegrationQueue, PostgresSearchIndexer,
        PostgresStripeRepository,
    },
    adapters::security::provider_secrets::DynamicSecretResolver,
    runtime::{clock::SystemClock, config::Settings, state::AppState},
};
use chaos_core::{
    analytics::AnalyticsDeliveryWorker,
    contracts::{
        AnalyticsEventDestination, Clock, IntegrationQueue, StripeAccountReadiness,
        StripePaymentGateway,
    },
    payments::PaymentWorkers,
};

/// Dependencies used by durable polling loops, without HTTP or MCP state.
#[derive(Clone)]
pub struct WorkerRuntime {
    pub payment_workers: Arc<PaymentWorkers>,
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

        let payment_repository =
            Arc::new(PostgresStripeRepository::new(infrastructure.runtime_pool()));
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

        Ok(Self {
            payment_workers: Arc::new(payment_workers),
            analytics_delivery_worker,
            search_indexer: Arc::new(PostgresSearchIndexer::new(infrastructure.runtime_pool())),
            clock: Arc::new(SystemClock),
        })
    }
}
