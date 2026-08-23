//! Use cases and ports. Infrastructure adapters implement ports defined here.

pub mod analytics;
pub mod catalog;
mod error;
pub mod identity;
pub mod inventory;
pub mod ports;
pub mod pricing;
pub mod sales;
pub mod shipping_events;
pub mod store;
pub mod storefront;
pub mod stripe;

pub use error::ApplicationError;
