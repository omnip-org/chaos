use async_trait::async_trait;
use chaos_domain::{
    catalog::{CollectionContent, CollectionId, CollectionStatus, ProductId},
    merchant::{SalesChannelId, StoreId},
};
use time::OffsetDateTime;

use crate::{ApplicationError, merchant::MerchantActor};

use super::{IdempotencyRequest, MachineActor};

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
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct StorefrontCollectionItem {
    pub id: CollectionId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub product_count: u32,
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

#[async_trait]
pub trait CollectionRepository: Send + Sync {
    async fn create(
        &self,
        actor: MerchantActor,
        record: CreateCollectionRecord,
        request: &IdempotencyRequest,
    ) -> Result<CollectionId, ApplicationError>;

    async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<CollectionId>,
        limit: u16,
    ) -> Result<Option<Vec<CollectionListItem>>, ApplicationError>;

    async fn get(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        collection_id: CollectionId,
    ) -> Result<Option<CollectionDetail>, ApplicationError>;

    async fn update(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        collection_id: CollectionId,
        content: &CollectionContent,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError>;

    async fn set_status(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        collection_id: CollectionId,
        status: CollectionStatus,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError>;

    async fn replace_products(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        collection_id: CollectionId,
        product_ids: &[ProductId],
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<CollectionId, ApplicationError>;

    async fn set_publication(
        &self,
        actor: MerchantActor,
        record: CollectionPublicationRecord,
        request: &IdempotencyRequest,
    ) -> Result<CollectionId, ApplicationError>;

    async fn list_storefront(
        &self,
        actor: &MachineActor,
        after: Option<CollectionId>,
        limit: u16,
    ) -> Result<Vec<StorefrontCollectionItem>, ApplicationError>;

    async fn get_storefront_by_handle(
        &self,
        actor: &MachineActor,
        handle: &str,
    ) -> Result<Option<StorefrontCollectionItem>, ApplicationError>;
}
