use async_trait::async_trait;
use chaos_domain::{
    catalog::{
        ProductId, ProductOptionId, ProductOptionValueId, ProductStatus, ProductVariantId,
        VariantStatus,
    },
    merchant::StoreId,
};
use time::OffsetDateTime;

use crate::{ApplicationError, ports::AdminActor};

pub struct CatalogProductListItem {
    pub id: ProductId,
    pub handle: String,
    pub title: String,
    pub status: ProductStatus,
    pub variant_count: u32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CatalogProductOptionValue {
    pub id: ProductOptionValueId,
    pub value: String,
    pub position: u16,
}

pub struct CatalogProductOption {
    pub id: ProductOptionId,
    pub name: String,
    pub position: u16,
    pub values: Vec<CatalogProductOptionValue>,
}

pub struct CatalogSelectedOption {
    pub option_id: ProductOptionId,
    pub option_name: String,
    pub option_value_id: ProductOptionValueId,
    pub value: String,
}

pub struct CatalogProductVariant {
    pub id: ProductVariantId,
    pub title: String,
    pub sku: Option<String>,
    pub status: VariantStatus,
    pub requires_shipping: bool,
    pub track_inventory: bool,
    pub selected_options: Vec<CatalogSelectedOption>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CatalogProductDetail {
    pub id: ProductId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub status: ProductStatus,
    pub options: Vec<CatalogProductOption>,
    pub variants: Vec<CatalogProductVariant>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[async_trait]
pub trait CatalogReadRepository: Send + Sync {
    async fn list_products(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<ProductId>,
        limit: u16,
    ) -> Result<Option<Vec<CatalogProductListItem>>, ApplicationError>;

    async fn get_product(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Option<CatalogProductDetail>, ApplicationError>;
}
