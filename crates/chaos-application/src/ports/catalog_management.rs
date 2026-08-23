use async_trait::async_trait;
use chaos_domain::{
    catalog::{ProductContent, ProductId, ProductStatus, ProductVariantContent, ProductVariantId},
    store::{SalesChannelId, StoreId},
};

use super::AdminActor;
use crate::ApplicationError;

pub struct ProductLifecycleSnapshot {
    pub status: ProductStatus,
    pub variant_count: u32,
}

#[async_trait]
pub trait CatalogManagementUnitOfWork: Send + Sync {
    async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Box<dyn CatalogManagementTransaction>, ApplicationError>;
}

#[async_trait]
pub trait CatalogManagementTransaction: Send {
    async fn load_lifecycle(
        &mut self,
    ) -> Result<Option<ProductLifecycleSnapshot>, ApplicationError>;

    async fn update_content(&mut self, content: &ProductContent) -> Result<bool, ApplicationError>;

    async fn update_variant_content(
        &mut self,
        variant_id: ProductVariantId,
        content: &ProductVariantContent,
    ) -> Result<bool, ApplicationError>;

    async fn set_status(&mut self, status: ProductStatus) -> Result<(), ApplicationError>;

    async fn active_channel_exists(
        &mut self,
        sales_channel_id: SalesChannelId,
    ) -> Result<bool, ApplicationError>;

    async fn publish(&mut self, sales_channel_id: SalesChannelId) -> Result<(), ApplicationError>;

    async fn unpublish(&mut self, sales_channel_id: SalesChannelId)
    -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}
