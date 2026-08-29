use chaos_domain::catalog::CatalogMetadata;

use crate::ApplicationError;

mod collections;
mod create_product;
mod management;
mod media;
mod queries;
mod reviews;
mod storefront;

pub(crate) fn parse_metadata(
    value: Option<serde_json::Value>,
) -> Result<Option<CatalogMetadata>, ApplicationError> {
    value
        .map(|value| {
            if !value.is_object() {
                return Err(ApplicationError::Validation {
                    violations: vec![chaos_domain::FieldViolation {
                        field: "metadata",
                        reason: "must be a JSON object; nested arrays and values are allowed"
                            .into(),
                    }],
                });
            }
            let text = serde_json::to_string(&value)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?;
            Ok(CatalogMetadata::parse(text)?)
        })
        .transpose()
}

pub(crate) fn parse_json_pointer(value: &str) -> Result<Vec<String>, ApplicationError> {
    if value.chars().count() > 512
        || value.len() < 2
        || !value.starts_with('/')
        || value.chars().any(char::is_control)
    {
        return Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "meta_path",
                reason: "must be a non-root RFC 6901 JSON Pointer of at most 512 characters without control characters".into(),
            }],
        });
    }
    value
        .split('/')
        .skip(1)
        .map(|segment| {
            if segment.is_empty() {
                return Err(ApplicationError::Validation {
                    violations: vec![chaos_domain::FieldViolation {
                        field: "meta_path",
                        reason: "must not contain empty path segments".into(),
                    }],
                });
            }
            let mut decoded = String::with_capacity(segment.len());
            let mut chars = segment.chars();
            while let Some(character) = chars.next() {
                if character != '~' {
                    decoded.push(character);
                    continue;
                }
                match chars.next() {
                    Some('0') => decoded.push('~'),
                    Some('1') => decoded.push('/'),
                    _ => {
                        return Err(ApplicationError::Validation {
                            violations: vec![chaos_domain::FieldViolation {
                                field: "meta_path",
                                reason: "must use valid RFC 6901 escape sequences".into(),
                            }],
                        });
                    }
                }
            }
            if decoded.is_empty() {
                return Err(ApplicationError::Validation {
                    violations: vec![chaos_domain::FieldViolation {
                        field: "meta_path",
                        reason: "must not contain empty path segments".into(),
                    }],
                });
            }
            Ok(decoded)
        })
        .collect()
}

pub use collections::{
    ChangeCollectionStatusInput, CollectionAdministration, CollectionPublicationInput,
    CreateCollectionInput, ReplaceCollectionProductsInput, StorefrontCollections,
    UpdateCollectionInput,
};
pub use create_product::{
    CreateProduct, CreateProductInput, CreateProductOptionInput, CreateProductOutput,
    CreateProductSelectedOptionInput, CreateProductVariantInput,
};
pub use management::{
    CatalogManagement, ChangeProductStatusInput, ProductPublicationInput, UpdateProductInput,
    UpdateProductVariantInput,
};
pub use media::{
    ArchiveMediaAssetInput, ArchiveProductMediaInput, ArchiveProductMetaMediaInput,
    ArchiveProductOptionValueMediaInput, ArchiveProductVariantMediaInput, ArchiveReviewMediaInput,
    AttachProductMediaInput, AttachProductMetaMediaInput, AttachProductOptionValueMediaInput,
    AttachProductVariantMediaInput, AttachReviewMediaInput, CompleteMediaUploadInput,
    CreateMediaUploadInput, CreatedMediaAsset, MediaAdministration, ProductMediaItemInput,
    RefreshMediaUploadInput, ReplaceProductMediaInput, ReplaceProductOptionValueMediaInput,
    ReplaceProductVariantMediaInput,
};
pub use queries::{CatalogQueries, ProductPage};
pub use reviews::{
    AddReviewReplyInput, ApproveReviewInput, CreateManualReviewInput, RejectReviewInput,
    ReviewAdministration, StorefrontReviews, SubmitReviewInput,
};
pub use storefront::{StorefrontCatalog, StorefrontProductPage};

#[cfg(test)]
mod tests {
    use super::{ApplicationError, parse_metadata};
    use serde_json::json;

    #[test]
    fn metadata_requires_an_object_root() {
        assert!(matches!(
            parse_metadata(Some(json!([]))),
            Err(ApplicationError::Validation { .. })
        ));
        assert!(matches!(
            parse_metadata(Some(json!(r#"{"key":"value"}"#))),
            Err(ApplicationError::Validation { .. })
        ));
        assert!(parse_metadata(Some(json!({ "tags": ["featured"] }))).is_ok());
        assert!(parse_metadata(None).unwrap().is_none());
    }
}
