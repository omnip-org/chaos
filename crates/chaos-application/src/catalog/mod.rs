mod create_product;
mod management;
mod queries;

pub use create_product::{
    CreateProduct, CreateProductInput, CreateProductOptionInput, CreateProductOutput,
    CreateProductSelectedOptionInput, CreateProductVariantInput,
};
pub use management::{
    CatalogManagement, ChangeProductStatusInput, ProductPublicationInput, UpdateProductInput,
};
pub use queries::{CatalogQueries, ProductPage};
