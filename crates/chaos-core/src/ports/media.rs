use async_trait::async_trait;
use chaos_domain::{
    catalog::{
        MediaAssetId, MediaAssetStatus, MediaDescriptor, MediaKind, ProductId, ProductVariantId,
    },
    store::StoreId,
};
use time::OffsetDateTime;

use crate::ApplicationError;

pub struct MediaAssetItem {
    pub id: MediaAssetId,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: Option<ProductVariantId>,
    pub file_name: String,
    pub media_type: String,
    pub kind: MediaKind,
    pub byte_size: u64,
    pub sha256_hex: String,
    pub alt_text: String,
    pub position: u16,
    pub status: MediaAssetStatus,
    pub public_url: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CreateMediaAssetRecord {
    pub id: MediaAssetId,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: Option<ProductVariantId>,
    pub descriptor: MediaDescriptor,
    pub object_key: String,
    pub position: u16,
    pub created_at: OffsetDateTime,
}

pub struct PendingMediaUpload {
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
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub changed_at: OffsetDateTime,
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
