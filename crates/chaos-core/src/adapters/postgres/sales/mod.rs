//! Order management and public storefront sales repositories.

mod inventory;
mod order_detail;
mod order_management;
mod storefront_catalog;
mod storefront_sales;

pub(crate) use inventory::{consume_order_inventory, release_order_inventory};
pub use order_management::PostgresOrderManagementRepository;
pub use storefront_catalog::PostgresStorefrontCatalogRepository;
pub use storefront_sales::PostgresStorefrontSalesRepository;
