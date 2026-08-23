use chaos_domain::{
    catalog::{CollectionContent, CollectionId, CollectionStatus, ProductId},
    store::{SalesChannelId, StoreId},
};
use time::OffsetDateTime;

pub struct CollectionListItem {
    pub id: CollectionId,
    pub handle: String,
    pub title: String,
    pub status: CollectionStatus,
    pub product_count: u32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CollectionProductItem {
    pub product_id: ProductId,
    pub position: u32,
}

pub struct CollectionDetail {
    pub id: CollectionId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub status: CollectionStatus,
    pub products: Vec<CollectionProductItem>,
    pub published_sales_channel_ids: Vec<SalesChannelId>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct StorefrontCollectionItem {
    pub id: CollectionId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub product_count: u32,
    pub metadata: Option<serde_json::Value>,
}

pub struct CreateCollectionRecord {
    pub id: CollectionId,
    pub store_id: StoreId,
    pub content: CollectionContent,
    pub created_at: OffsetDateTime,
}

pub struct CollectionPublicationRecord {
    pub store_id: StoreId,
    pub collection_id: CollectionId,
    pub sales_channel_id: SalesChannelId,
    pub published: bool,
    pub changed_at: OffsetDateTime,
}
