use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode, RegionCode,
    store::{
        SalesChannel, SalesChannelCode, SalesChannelId, SalesChannelStatus, Store, StoreCode,
        StoreId, StoreStatus,
    },
};
use time::OffsetDateTime;

use crate::ApplicationError;

use super::AdminActor;

pub struct StoreAdminItem {
    pub id: StoreId,
    pub code: StoreCode,
    pub name: String,
    pub region: RegionCode,
    pub currency: CurrencyCode,
    pub meta: Option<serde_json::Value>,
    pub status: StoreStatus,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct SalesChannelAdminItem {
    pub id: SalesChannelId,
    pub code: SalesChannelCode,
    pub name: String,
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
    ) -> Result<StoreId, ApplicationError>;

    async fn change_store_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        status: StoreStatus,
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
    ) -> Result<SalesChannelId, ApplicationError>;

    async fn update_sales_channel(
        &self,
        actor: AdminActor,
        sales_channel_id: SalesChannelId,
        replacement: &SalesChannel,
    ) -> Result<SalesChannelId, ApplicationError>;

    async fn change_sales_channel_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        status: SalesChannelStatus,
    ) -> Result<SalesChannelId, ApplicationError>;
}
