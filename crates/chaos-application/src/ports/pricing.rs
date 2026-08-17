use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    catalog::ProductVariantId,
    merchant::StoreId,
    pricing::{PriceList, PriceListId},
};

use super::{AdminActor, IdempotencyRequest};
use crate::ApplicationError;

#[async_trait]
pub trait PricingProvisioningUnitOfWork: Send + Sync {
    async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Box<dyn PricingProvisioningTransaction>, ApplicationError>;
}

#[async_trait]
pub trait PricingProvisioningTransaction: Send {
    async fn require_writable_store(&mut self) -> Result<(), ApplicationError>;

    async fn require_enabled_currency(
        &mut self,
        currency: CurrencyCode,
    ) -> Result<(), ApplicationError>;

    async fn reserve_price_list_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<PriceListId>, ApplicationError>;

    async fn active_variant_ids(
        &mut self,
        variant_ids: &[ProductVariantId],
    ) -> Result<Vec<ProductVariantId>, ApplicationError>;

    async fn store_variant_ids(
        &mut self,
        variant_ids: &[ProductVariantId],
    ) -> Result<Vec<ProductVariantId>, ApplicationError>;

    async fn insert_price_list(&mut self, price_list: &PriceList) -> Result<(), ApplicationError>;

    async fn complete_price_list_creation(
        &mut self,
        request: &IdempotencyRequest,
        price_list_id: PriceListId,
    ) -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}
