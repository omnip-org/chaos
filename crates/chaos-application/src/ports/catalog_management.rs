use async_trait::async_trait;
use chaos_domain::{
    catalog::{ProductContent, ProductId, ProductStatus, ProductVariantContent, ProductVariantId},
    store::{SalesChannelId, StoreId},
};

use super::{AdminActor, IdempotencyRequest};
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
    async fn reserve_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
    ) -> Result<Option<ProductId>, ApplicationError>;

    async fn reserve_variant_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
    ) -> Result<Option<ProductVariantId>, ApplicationError>;

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

    async fn complete_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
        product_id: ProductId,
    ) -> Result<(), ApplicationError>;

    async fn complete_variant_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
        variant_id: ProductVariantId,
    ) -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}
