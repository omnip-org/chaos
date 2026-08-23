//! Core business use cases and their runtime adapters.

pub mod adapters;
pub mod analytics;
pub mod catalog;
pub mod contracts;
pub mod identity;
pub mod inventory;
pub mod payments;
pub mod pricing;
pub mod runtime;
pub mod sales;
pub mod store;

mod error;

pub use error::ApplicationError;
