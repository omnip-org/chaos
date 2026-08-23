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
    PostgresCatalogManagementUnitOfWork, PostgresCatalogProvisioningUnitOfWork,
    PostgresCatalogReadRepository, PostgresCollectionRepository, PostgresMediaAssetRepository,
    PostgresReviewRepository,
};
pub use integration::{PostgresIntegrationQueue, PostgresShippingEventRepository};
pub use inventory::PostgresInventoryRepository;
pub use pricing::{PostgresPricingManagementRepository, PostgresPricingProvisioningUnitOfWork};
pub use sales::{
    PostgresOrderManagementRepository, PostgresStorefrontCatalogRepository,
    PostgresStorefrontSalesRepository,
};
pub use search::PostgresSearchIndexer;
pub use store::{
    DefaultPublishableKeyGenerator, PostgresPublishableKeyRepository,
    PostgresStoreAdministrationRepository, PostgresStoreMembershipRepository,
    PostgresStoreProvisioningUnitOfWork, PostgresStoreReadRepository,
};
pub use stripe::PostgresStripeRepository;
