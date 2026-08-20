use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode, RegionCode,
    identity::UserId,
    store::{StoreCode, StoreId, StoreRole, StoreStatus},
};

use crate::ApplicationError;

pub struct StoreListItem {
    pub id: StoreId,
    pub code: StoreCode,
    pub name: String,
    pub default_region: RegionCode,
    pub default_currency: CurrencyCode,
    pub status: StoreStatus,
    pub role: StoreRole,
}

#[async_trait]
pub trait StoreReadRepository: Send + Sync {
    async fn membership_role(
        &self,
        user_id: UserId,
        store_id: StoreId,
    ) -> Result<Option<StoreRole>, ApplicationError>;

    async fn list_stores(
        &self,
        user_id: UserId,
        after: Option<StoreId>,
        limit: u16,
    ) -> Result<Vec<StoreListItem>, ApplicationError>;
}
