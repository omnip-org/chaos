//! Pure business rules. This crate must not depend on web frameworks, databases,
//! caches, serialization formats, or other delivery mechanisms.

pub mod catalog;
pub mod identity;
pub mod inventory;
pub mod payments;
pub mod pricing;
pub mod sales;
pub mod store;

mod currency;
mod locale;
mod region;

pub use currency::CurrencyCode;
pub use locale::Locale;
pub use region::RegionCode;

/// Upper bound for opaque secret-manager reference strings (`env://...`, `enc://...`),
/// sized to cover a base64-encoded, AES-256-GCM-sealed 16KB Provider Key value
/// (nonce + ciphertext + tag), with margin.
pub const MAX_SECRET_REFERENCE_LEN: usize = 32_768;

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
