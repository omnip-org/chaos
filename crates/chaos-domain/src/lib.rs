//! Pure business rules. This crate must not depend on web frameworks, databases,
//! caches, serialization formats, or other delivery mechanisms.

pub mod analytics;
pub mod catalog;
pub mod fulfillment;
pub mod identity;
pub mod inventory;
pub mod merchant;
pub mod payments;
pub mod pricing;
pub mod sales;

mod currency;
mod region;

pub use currency::CurrencyCode;
pub use region::RegionCode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldViolation {
    pub field: &'static str,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("domain validation failed")]
    Validation(Vec<FieldViolation>),
}
