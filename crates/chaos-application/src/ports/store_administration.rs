use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode, RegionCode,
    store::{
        SalesChannel, SalesChannelCode, SalesChannelId, SalesChannelKind, SalesChannelStatus,
        Store, StoreCode, StoreId, StoreStatus,
    },
};
use time::OffsetDateTime;

use crate::ApplicationError;

use super::{AdminActor, IdempotencyRequest};

pub struct StoreAdminItem {
    pub id: StoreId,
    pub code: StoreCode,
    pub name: String,
    pub default_region: RegionCode,
    pub default_currency: CurrencyCode,
    pub status: StoreStatus,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct SalesChannelAdminItem {
    pub id: SalesChannelId,
    pub code: SalesChannelCode,
    pub name: String,
    pub kind: SalesChannelKind,
    pub status: SalesChannelStatus,
    pub is_default: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[async_trait]
pub trait StoreAdministrationRepository: Send + Sync {
    async fn get_store(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Option<StoreAdminItem>, ApplicationError>;

    async fn update_store(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        replacement: &Store,
        request: &IdempotencyRequest,
    ) -> Result<StoreId, ApplicationError>;

    async fn change_store_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        status: StoreStatus,
        request: &IdempotencyRequest,
    ) -> Result<StoreId, ApplicationError>;

    async fn list_sales_channels(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<SalesChannelId>,
        limit: u16,
    ) -> Result<Option<Vec<SalesChannelAdminItem>>, ApplicationError>;

    async fn get_sales_channel(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
    ) -> Result<Option<SalesChannelAdminItem>, ApplicationError>;

    async fn create_sales_channel(
        &self,
        actor: AdminActor,
        channel: &SalesChannel,
        request: &IdempotencyRequest,
    ) -> Result<SalesChannelId, ApplicationError>;

    async fn update_sales_channel(
        &self,
        actor: AdminActor,
        sales_channel_id: SalesChannelId,
        replacement: &SalesChannel,
        request: &IdempotencyRequest,
    ) -> Result<SalesChannelId, ApplicationError>;

    async fn change_sales_channel_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        status: SalesChannelStatus,
        request: &IdempotencyRequest,
    ) -> Result<SalesChannelId, ApplicationError>;
}
