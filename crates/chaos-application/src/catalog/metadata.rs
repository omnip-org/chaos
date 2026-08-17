use chaos_domain::catalog::CatalogMetadata;

use crate::ApplicationError;

/// Converts a Storefront/Admin-supplied JSON value into the domain's bounded
/// metadata representation. Serializing back to canonical text here (rather than
/// passing the raw request body through) guarantees the stored text is always
/// well-formed JSON, so infrastructure can bind it with a `::jsonb` cast without
/// re-validating syntax.
pub fn parse_metadata(
    value: Option<serde_json::Value>,
) -> Result<Option<CatalogMetadata>, ApplicationError> {
    value
        .map(|value| {
            let text = serde_json::to_string(&value)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?;
            Ok(CatalogMetadata::parse(text)?)
        })
        .transpose()
}
