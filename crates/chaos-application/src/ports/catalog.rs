use async_trait::async_trait;
use chaos_domain::{
    catalog::{Product, ProductId},
    merchant::StoreId,
};

use super::{AdminActor, IdempotencyRequest};
use crate::ApplicationError;

#[async_trait]
pub trait CatalogProvisioningUnitOfWork: Send + Sync {
    async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Box<dyn CatalogProvisioningTransaction>, ApplicationError>;
}

#[async_trait]
pub trait CatalogProvisioningTransaction: Send {
    async fn require_writable_store(&mut self) -> Result<(), ApplicationError>;

    async fn reserve_product_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<ProductId>, ApplicationError>;

    async fn insert_product(&mut self, product: &Product) -> Result<(), ApplicationError>;

    async fn complete_product_creation(
        &mut self,
        request: &IdempotencyRequest,
        product_id: ProductId,
    ) -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}
