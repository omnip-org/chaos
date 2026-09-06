//! Runtime-specific dependency composition.

use std::sync::Arc;

use chaos_core::{
    adapters::integrations::{
        analytics::meta::MetaConversionsDestination, resend::ResendEmailProvider,
        stripe::StripeGateway,
    },
    adapters::postgres::{
        PostgresCapiEventStore, PostgresEmailRepository, PostgresIntegrationQueue,
        PostgresMaintenance, PostgresProviderWebhookAudit, PostgresSearchIndexer,
        PostgresStripeRepository,
    },
    adapters::security::provider_secrets::DynamicSecretResolver,
    runtime::{clock::SystemClock, config::Settings, state::AppState},
};
use chaos_core::{
    analytics::MetaCapiWorker,
    contracts::{
        AnalyticsEventDestination, Clock, EmailProvider, IntegrationQueue, PaymentProvider,
        PaymentProviderRegistry,
    },
    email::EmailWorkers,
    webhooks::ProviderWebhookWorker,
};

/// Dependencies used by durable polling loops, without HTTP or MCP state.
#[derive(Clone)]
pub struct WorkerRuntime {
    pub email_workers: Arc<EmailWorkers>,
    pub provider_webhook_worker: Arc<ProviderWebhookWorker>,
    pub capi_worker: Arc<MetaCapiWorker>,
    pub search_indexer: Arc<PostgresSearchIndexer>,
    pub maintenance: Arc<PostgresMaintenance>,
    pub clock: Arc<dyn Clock>,
}

impl WorkerRuntime {
    pub fn new(infrastructure: &AppState, settings: &Settings) -> anyhow::Result<Self> {
        let dynamic_secrets = Arc::new(DynamicSecretResolver::new(&settings.provider_secret_key));

        let integration_queue: Arc<dyn IntegrationQueue> =
            Arc::new(PostgresIntegrationQueue::new(infrastructure.runtime_pool()));

        let capi_event_store = Arc::new(PostgresCapiEventStore::new(infrastructure.runtime_pool()));
        let meta_destination = Arc::new(MetaConversionsDestination::new(
            settings.analytics_meta_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let capi_worker = Arc::new(MetaCapiWorker::new(
            integration_queue.clone(),
            capi_event_store,
            meta_destination as Arc<dyn AnalyticsEventDestination>,
        ));

        let stripe_gateway = Arc::new(StripeGateway::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let payment_providers = Arc::new(PaymentProviderRegistry::new([
            stripe_gateway as Arc<dyn PaymentProvider>
        ]));
        let provider_webhook_worker = Arc::new(ProviderWebhookWorker::new(
            integration_queue.clone(),
            Arc::new(PostgresProviderWebhookAudit::new(
                infrastructure.runtime_pool(),
            )),
            Arc::new(PostgresStripeRepository::new(infrastructure.runtime_pool())),
            payment_providers,
        ));

        let email_provider = Arc::new(ResendEmailProvider::new(
            settings.resend_api_base_url.clone(),
            dynamic_secrets.clone(),
            settings.dependency_timeout,
        )?) as Arc<dyn EmailProvider>;
        let email_workers = EmailWorkers::new(
            integration_queue,
            Arc::new(PostgresEmailRepository::new(infrastructure.runtime_pool())),
            [email_provider],
        );

        Ok(Self {
            email_workers: Arc::new(email_workers),
            provider_webhook_worker,
            capi_worker,
            search_indexer: Arc::new(PostgresSearchIndexer::new(infrastructure.runtime_pool())),
            maintenance: Arc::new(PostgresMaintenance::new(
                infrastructure.runtime_pool(),
                infrastructure.identity_pool(),
            )),
            clock: Arc::new(SystemClock),
        })
    }
}
