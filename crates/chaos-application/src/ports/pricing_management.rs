use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    catalog::ProductVariantId,
    merchant::StoreId,
    pricing::{PriceList, PriceListId, PriceListStatus},
};
use time::OffsetDateTime;

use crate::ApplicationError;

use super::{AdminActor, IdempotencyRequest};

pub struct PriceListReadItem {
    pub id: PriceListId,
    pub code: String,
    pub name: String,
    pub currency: CurrencyCode,
    pub tax_inclusive: bool,
    pub status: PriceListStatus,
    pub starts_at: Option<OffsetDateTime>,
    pub ends_at: Option<OffsetDateTime>,
    pub price_count: u32,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct PriceReadItem {
    pub product_variant_id: ProductVariantId,
    pub amount_minor: i64,
}

pub struct PriceListDetail {
    pub item: PriceListReadItem,
    pub prices: Vec<PriceReadItem>,
}

pub struct PriceListMutationSnapshot {
    pub status: PriceListStatus,
    pub priced_variant_ids: Vec<ProductVariantId>,
}

#[async_trait]
pub trait PricingReadRepository: Send + Sync {
    async fn list_price_lists(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<PriceListId>,
        limit: u16,
    ) -> Result<Option<Vec<PriceListReadItem>>, ApplicationError>;

    async fn get_price_list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        price_list_id: PriceListId,
    ) -> Result<Option<PriceListDetail>, ApplicationError>;
}

#[async_trait]
pub trait PricingManagementUnitOfWork: Send + Sync {
    async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        price_list_id: PriceListId,
    ) -> Result<Box<dyn PricingManagementTransaction>, ApplicationError>;
}

#[async_trait]
pub trait PricingManagementTransaction: Send {
    async fn reserve_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
    ) -> Result<Option<PriceListId>, ApplicationError>;

    async fn load_for_update(
        &mut self,
    ) -> Result<Option<PriceListMutationSnapshot>, ApplicationError>;

    async fn active_variant_ids(
        &mut self,
        variant_ids: &[ProductVariantId],
    ) -> Result<Vec<ProductVariantId>, ApplicationError>;

    async fn store_variant_ids(
        &mut self,
        variant_ids: &[ProductVariantId],
    ) -> Result<Vec<ProductVariantId>, ApplicationError>;

    async fn currency_is_enabled(
        &mut self,
        currency: CurrencyCode,
    ) -> Result<bool, ApplicationError>;

    async fn replace(&mut self, price_list: &PriceList) -> Result<(), ApplicationError>;

    async fn set_status(&mut self, status: PriceListStatus) -> Result<(), ApplicationError>;

    async fn complete_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
        price_list_id: PriceListId,
    ) -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}
