use chaos_domain::catalog::CatalogMetadata;

use crate::ApplicationError;

mod collections;
mod create_product;
mod management;
mod media;
mod queries;
mod reviews;

pub(crate) fn parse_metadata(
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
    CreateMediaAssetInput, CreatedMediaAsset, MediaAdministration, MediaAssetActionInput,
    RefreshMediaUploadInput,
};
pub use queries::{CatalogQueries, ProductPage};
pub use reviews::{
    AddReviewReplyInput, ApproveReviewInput, RejectReviewInput, ReviewAdministration,
    StorefrontReviews, SubmitReviewInput,
};
