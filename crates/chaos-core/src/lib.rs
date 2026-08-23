//! Core business use cases, persistence, and external adapters.

pub mod analytics;
pub mod catalog;
pub mod database;
mod error;
pub mod identity;
pub mod integrations;
pub mod inventory;
pub mod ports;
pub mod pricing;
pub mod repositories;
pub mod runtime;
pub mod sales;
pub mod security;
pub mod shipping_events;
pub mod storage;
pub mod store;
pub mod storefront;
pub mod stripe;

pub use error::ApplicationError;
