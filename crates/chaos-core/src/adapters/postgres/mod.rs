//! PostgreSQL repositories grouped by business capability.

mod analytics;
mod catalog;
pub(crate) mod database;
mod fulfillment;
mod integrations;
mod inventory;
mod maintenance;
mod payments;
mod pricing;
mod sales;
mod search;
mod store;

pub use analytics::{
    PostgresAnalyticsDeliveryStore, PostgresAnalyticsDestinationStore, PostgresAnalyticsEventStore,
};
pub use catalog::{
    PostgresCatalogConfigurationRepository, PostgresCatalogManagementRepository,
    PostgresCatalogProvisioningRepository, PostgresCatalogReadRepository,
    PostgresCollectionRepository, PostgresMediaAssetRepository, PostgresReviewRepository,
};
pub use fulfillment::PostgresFulfillmentRepository;
pub(crate) use integrations::{EmailBrandWrite, EmailProviderAccountWrite};
pub use integrations::{
    PostgresEmailRepository, PostgresIntegrationAccountRepository, PostgresIntegrationQueue,
    PostgresIntegrationWebhookRepository, PostgresShippingRepository,
};
pub use inventory::PostgresInventoryRepository;
pub use maintenance::PostgresMaintenance;
pub(crate) use payments::OrderCheckoutPayment;
pub use payments::PostgresStripeRepository;
pub use pricing::{PostgresPricingManagementRepository, PostgresPricingProvisioningRepository};
pub use sales::{
    PostgresOrderManagementRepository, PostgresStorefrontCatalogRepository,
    PostgresStorefrontSalesRepository,
};
pub use search::PostgresSearchIndexer;
pub use store::{
    DefaultPublishableKeyGenerator, PostgresPublishableKeyRepository,
    PostgresStoreAdministrationRepository, PostgresStoreMembershipRepository,
    PostgresStoreProvisioningRepository, PostgresStoreReadRepository,
};
