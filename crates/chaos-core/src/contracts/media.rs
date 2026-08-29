use async_trait::async_trait;
use chaos_domain::{
    catalog::{
        MediaAssetId, MediaAssetStatus, MediaDescriptor, MediaKind, ProductId, ProductOptionId,
        ProductOptionValueId, ProductVariantId, ReviewId,
    },
    store::StoreId,
};
use time::OffsetDateTime;

use crate::ApplicationError;

/// The provider-independent metadata and lifecycle state of one physical object.
///
/// This type deliberately has no business target. Product galleries, review images,
/// and metadata content all point at the same verified object record.
#[derive(Clone)]
pub struct MediaAssetItem {
    pub id: MediaAssetId,
    pub store_id: StoreId,
    pub file_name: String,
    pub media_type: String,
    pub kind: MediaKind,
    pub byte_size: u64,
    pub sha256_hex: String,
    pub status: MediaAssetStatus,
    pub public_url: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct ProductMediaAssetItem {
    pub asset: MediaAssetItem,
    pub product_id: ProductId,
    pub scope: ProductMediaScope,
    pub alt_text: String,
    pub position: u16,
    pub archived_at: Option<OffsetDateTime>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductMediaScope {
    Product,
    OptionValue {
        option_id: ProductOptionId,
        option_value_id: ProductOptionValueId,
    },
    Variant {
        product_variant_id: ProductVariantId,
    },
}

pub struct ReviewMediaAssetItem {
    pub asset: MediaAssetItem,
    pub review_id: ReviewId,
    pub alt_text: String,
    pub position: u16,
    pub archived_at: Option<OffsetDateTime>,
}

pub struct ProductMetaMediaAssetItem {
    pub asset: MediaAssetItem,
    pub product_id: ProductId,
    /// RFC 6901 JSON Pointer into the Product metadata object.
    pub meta_path: String,
    pub alt_text: String,
    pub archived_at: Option<OffsetDateTime>,
}

pub struct CreateMediaAssetRecord {
    pub id: MediaAssetId,
    pub store_id: StoreId,
    pub descriptor: MediaDescriptor,
    pub object_key: String,
    pub created_at: OffsetDateTime,
}

/// A database asset plus its server-owned object key. The object key never crosses
/// the model-facing MCP response; it is only used by the storage adapter.
pub struct MediaAssetStorageRecord {
    pub asset: MediaAssetItem,
    pub object_key: String,
}

pub struct MediaUploadRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub expires_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct StoredMediaObject {
    pub media_type: String,
    pub byte_size: u64,
    pub sha256_hex: String,
}

pub struct MediaAssetMutation {
    pub store_id: StoreId,
    pub media_asset_id: MediaAssetId,
    pub changed_at: OffsetDateTime,
}

pub struct ProductMediaAssetLinkRecord {
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub alt_text: String,
    pub position: u16,
    pub changed_at: OffsetDateTime,
}

pub struct ProductOptionValueMediaAssetLinkRecord {
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub option_id: ProductOptionId,
    pub option_value_id: ProductOptionValueId,
    pub media_asset_id: MediaAssetId,
    pub alt_text: String,
    pub position: u16,
    pub changed_at: OffsetDateTime,
}

pub struct ProductVariantMediaAssetLinkRecord {
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub media_asset_id: MediaAssetId,
    pub alt_text: String,
    pub position: u16,
    pub changed_at: OffsetDateTime,
}

pub struct ReviewMediaAssetLinkRecord {
    pub store_id: StoreId,
    pub review_id: ReviewId,
    pub media_asset_id: MediaAssetId,
    pub alt_text: String,
    pub position: u16,
}

pub struct ProductMetaMediaAssetLinkRecord {
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub meta_path: String,
    pub alt_text: String,
    pub changed_at: OffsetDateTime,
    pub expected_revision: Option<i64>,
}

pub struct ProductMediaAssetMutation {
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub changed_at: OffsetDateTime,
    pub expected_revision: Option<i64>,
}

pub struct ProductOptionValueMediaAssetMutation {
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub option_id: ProductOptionId,
    pub option_value_id: ProductOptionValueId,
    pub media_asset_id: MediaAssetId,
    pub changed_at: OffsetDateTime,
    pub expected_revision: Option<i64>,
}

pub struct ProductVariantMediaAssetMutation {
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub media_asset_id: MediaAssetId,
    pub changed_at: OffsetDateTime,
    pub expected_revision: Option<i64>,
}

pub struct ReviewMediaAssetMutation {
    pub store_id: StoreId,
    pub review_id: ReviewId,
    pub media_asset_id: MediaAssetId,
    pub changed_at: OffsetDateTime,
}

pub struct ProductMetaMediaAssetMutation {
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub meta_path: String,
    pub changed_at: OffsetDateTime,
    pub expected_revision: Option<i64>,
}

#[async_trait]
pub trait MediaStorage: Send + Sync {
    async fn prepare_upload(
        &self,
        object_key: &str,
        descriptor: &MediaDescriptor,
        valid_for: std::time::Duration,
        expires_at: OffsetDateTime,
    ) -> Result<MediaUploadRequest, ApplicationError>;

    async fn inspect(
        &self,
        object_key: &str,
    ) -> Result<Option<StoredMediaObject>, ApplicationError>;

    fn public_url(&self, object_key: &str) -> Result<String, ApplicationError>;
}
