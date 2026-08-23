//! PostgreSQL repositories grouped by business capability.

mod analytics;
mod catalog;
mod integration;
mod inventory;
mod payments;
mod pricing;
mod sales;
mod search;
mod shared;
mod shipping_events;
mod store;

pub use analytics::{
    PostgresAnalyticsDeliveryStore, PostgresAnalyticsDestinationStore, PostgresAnalyticsEventStore,
};
pub use catalog::{
    PostgresCatalogManagementUnitOfWork, PostgresCatalogProvisioningUnitOfWork,
    PostgresCatalogReadRepository, PostgresCollectionRepository, PostgresMediaAssetRepository,
    PostgresReviewRepository,
};
pub use integration::PostgresIntegrationQueue;
pub use inventory::PostgresInventoryRepository;
pub use payments::PostgresPaymentRepository;
pub use pricing::{PostgresPricingManagementRepository, PostgresPricingProvisioningUnitOfWork};
pub use sales::{
    PostgresOrderManagementRepository, PostgresStorefrontCatalogRepository,
    PostgresStorefrontSalesRepository,
};
pub use search::PostgresSearchIndexer;
pub use shipping_events::PostgresShippingEventRepository;
pub use store::{
    DefaultPublishableKeyGenerator, PostgresPublishableKeyRepository,
    PostgresStoreAdministrationRepository, PostgresStoreMembershipRepository,
    PostgresStoreProvisioningUnitOfWork, PostgresStoreReadRepository,
};
