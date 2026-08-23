//! PostgreSQL repositories grouped by business capability.

mod analytics;
mod catalog;
mod integration;
mod inventory;
mod pricing;
mod sales;
mod search;
mod shared;
mod store;
mod stripe;

pub use analytics::{
    PostgresAnalyticsDeliveryStore, PostgresAnalyticsDestinationStore, PostgresAnalyticsEventStore,
};
pub use catalog::{
    PostgresCatalogManagementRepository, PostgresCatalogProvisioningRepository,
    PostgresCatalogReadRepository, PostgresCollectionRepository, PostgresMediaAssetRepository,
    PostgresReviewRepository,
};
pub use integration::{PostgresIntegrationQueue, PostgresShippingEventRepository};
pub use inventory::PostgresInventoryRepository;
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
pub use stripe::PostgresStripeRepository;
