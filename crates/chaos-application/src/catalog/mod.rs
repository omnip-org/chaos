mod collections;
mod create_product;
mod management;
mod media;
mod queries;

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
};
pub use media::{
    CreateMediaAssetInput, CreatedMediaAsset, MediaAdministration, MediaAssetActionInput,
    RefreshMediaUploadInput,
};
pub use queries::{CatalogQueries, ProductPage};
