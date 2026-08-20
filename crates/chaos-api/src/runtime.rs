//! Runtime-specific dependency composition.

use std::sync::Arc;

use anyhow::Context as _;
use chaos_application::{
    analytics::AnalyticsWorkers,
    fulfillment::FulfillmentWorkers,
    notifications::NotificationWorkers,
    payments::PaymentWorkers,
    ports::{Clock, EmailProvider, PaymentProvider, PaymentProviderOnboarding, ShippingProvider},
    sales::CheckoutExpiryWorkers,
};
use chaos_infrastructure::{
    clock::SystemClock,
    config::Settings,
    meta::MetaConversionsDestination,
    providers::{
        easypost::EasyPostShippingProvider,
        email::{ResendEmailProvider, SmtpEmailProvider},
        stripe::{StripeCheckoutPaymentProvider, StripePaymentProvider},
    },
    repositories::{
        PostgresAnalyticsEventRepository, PostgresEmailDeliveryRepository,
        PostgresFulfillmentRepository, PostgresPaymentRepository, PostgresSearchIndexer,
        PostgresShippingServiceRepository, PostgresStorefrontSalesRepository,
        SandboxPaymentProvider,
    },
    secret::DynamicSecretResolver,
    state::AppState,
};

/// Dependencies used by durable polling loops, without HTTP or MCP state.
#[derive(Clone)]
pub struct WorkerRuntime {
    pub payment_workers: Arc<PaymentWorkers>,
    pub fulfillment_workers: Arc<FulfillmentWorkers>,
    pub notification_workers: Arc<NotificationWorkers>,
    pub analytics_workers: Arc<AnalyticsWorkers>,
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
        let analytics_workers = AnalyticsWorkers::new(analytics_repository, meta_destination);

        let payment_repository = Arc::new(PostgresPaymentRepository::new(
            infrastructure.runtime_pool(),
        ));
        let sandbox_payment_provider = Arc::new(SandboxPaymentProvider);
        let stripe_payment_provider = Arc::new(StripePaymentProvider::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let stripe_checkout_payment_provider = Arc::new(StripeCheckoutPaymentProvider::new(
            settings.stripe_api_base_url.clone(),
            settings.dependency_timeout,
            dynamic_secrets.clone(),
        )?);
        let payment_providers = vec![
            sandbox_payment_provider.clone() as Arc<dyn PaymentProvider>,
            stripe_payment_provider.clone() as Arc<dyn PaymentProvider>,
            stripe_checkout_payment_provider.clone() as Arc<dyn PaymentProvider>,
        ];
        let payment_onboarding = vec![
            sandbox_payment_provider as Arc<dyn PaymentProviderOnboarding>,
            stripe_payment_provider as Arc<dyn PaymentProviderOnboarding>,
            stripe_checkout_payment_provider as Arc<dyn PaymentProviderOnboarding>,
        ];
        let payment_workers = PaymentWorkers::new(
            payment_repository.clone(),
            payment_repository.clone(),
            payment_repository,
            payment_providers,
            payment_onboarding,
        );

        let email_provider: Arc<dyn EmailProvider> = if let Some(api_key) = &settings.resend_api_key
        {
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
        let notification_repository = Arc::new(PostgresEmailDeliveryRepository::new(
            infrastructure.runtime_pool(),
        ));
        let notification_providers = if email_provider.name() == "resend" {
            vec![email_provider]
        } else {
            Vec::new()
        };
        let notification_workers = NotificationWorkers::new(
            notification_repository,
            notification_providers,
            settings.email_from.clone(),
            settings.storefront_public_base_url.to_string(),
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
            payment_workers: Arc::new(payment_workers),
            fulfillment_workers: Arc::new(fulfillment_workers),
            notification_workers: Arc::new(notification_workers),
            analytics_workers: Arc::new(analytics_workers),
            search_indexer: Arc::new(PostgresSearchIndexer::new(infrastructure.runtime_pool())),
            checkout_expiry_workers: Arc::new(CheckoutExpiryWorkers::new(
                storefront_sales_repository,
            )),
            clock: Arc::new(SystemClock),
        })
    }
}
