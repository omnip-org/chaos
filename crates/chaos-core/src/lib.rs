//! Core business use cases and their runtime adapters.

pub mod adapters;
pub mod analytics;
pub mod catalog;
pub mod contracts;
pub mod email;
pub mod fulfillment;
pub mod identity;
pub mod inventory;
pub mod payments;
pub mod pricing;
pub mod runtime;
pub mod sales;
pub mod shipping;
pub mod store;

mod email_templates;
mod error;

pub use error::ApplicationError;
