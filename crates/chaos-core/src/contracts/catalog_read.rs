use crate::contracts::media::ProductMediaAssetItem;
use chaos_domain::catalog::{
    ProductId, ProductOptionId, ProductOptionValueId, ProductStatus, ProductVariantId,
    VariantStatus,
};
use time::OffsetDateTime;

pub struct CatalogProductListItem {
    pub id: ProductId,
    pub handle: String,
    pub title: String,
    pub status: ProductStatus,
    pub variant_count: u32,
    pub revision: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CatalogProductOptionValue {
    pub id: ProductOptionValueId,
    pub value: String,
    pub position: u16,
    pub archived_at: Option<OffsetDateTime>,
}

pub struct CatalogProductOption {
    pub id: ProductOptionId,
    pub name: String,
    pub position: u16,
    pub archived_at: Option<OffsetDateTime>,
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
    pub revision: i64,
    pub options: Vec<CatalogProductOption>,
    pub variants: Vec<CatalogProductVariant>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CatalogProductPublication {
    pub sales_channel_id: chaos_domain::store::SalesChannelId,
}

pub struct CatalogProductWorkspace {
    pub product: CatalogProductDetail,
    pub media: Vec<ProductMediaAssetItem>,
    pub publications: Vec<CatalogProductPublication>,
}
