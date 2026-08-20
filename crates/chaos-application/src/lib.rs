//! Use cases and ports. Infrastructure adapters implement ports defined here.

pub mod analytics;
pub mod catalog;
mod error;
pub mod fulfillment;
pub mod identity;
pub mod inventory;
pub mod notifications;
pub mod payments;
pub mod ports;
pub mod pricing;
pub mod sales;
pub mod store;
pub mod storefront;

pub use error::ApplicationError;
