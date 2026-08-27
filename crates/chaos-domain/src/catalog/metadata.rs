use crate::{DomainError, FieldViolation};

/// Bounded, opaque merchandising content attached to a Product, ProductVariant, or
/// Collection. The domain only enforces a byte bound; the application boundary
/// validates the JSON object root before constructing this type, and interpreting
/// the nested shape is entirely a Storefront-client concern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogMetadata(String);

impl CatalogMetadata {
    pub const MAX_BYTES: usize = 32_768;

    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_BYTES {
            return Err(DomainError::Validation(vec![FieldViolation {
                field: "metadata",
                reason: format!("must contain 1-{} bytes of JSON text", Self::MAX_BYTES),
            }]));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::CatalogMetadata;

    #[test]
    fn rejects_empty_and_oversized_metadata() {
        assert!(CatalogMetadata::parse("").is_err());
        assert!(CatalogMetadata::parse("x".repeat(CatalogMetadata::MAX_BYTES + 1)).is_err());
        assert!(CatalogMetadata::parse("{}").is_ok());
        assert!(CatalogMetadata::parse("x".repeat(CatalogMetadata::MAX_BYTES)).is_ok());
    }
}
