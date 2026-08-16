mod collection;
mod product;

pub use collection::{CollectionContent, CollectionHandle, CollectionId, CollectionStatus};
pub use product::{
    Product, ProductContent, ProductHandle, ProductId, ProductLifecycle, ProductOption,
    ProductOptionId, ProductOptionValue, ProductOptionValueId, ProductStatus, ProductVariant,
    ProductVariantId, SelectedOptionValue, Sku, VariantStatus,
};
