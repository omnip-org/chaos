use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode, RegionCode,
    identity::UserId,
    store::{StoreId, StoreRole, StoreStatus},
};
use time::OffsetDateTime;

use crate::{ApplicationError, store::StoreActor};

pub struct StoreListItem {
    pub id: StoreId,
    pub name: String,
    pub region: RegionCode,
    pub currency: CurrencyCode,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreMembershipItem {
    pub user_id: UserId,
    pub role: StoreRole,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[async_trait]
pub trait StoreMembershipRepository: Send + Sync {
    async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<Vec<StoreMembershipItem>, ApplicationError>;

    async fn add_member(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        user_id: UserId,
    ) -> Result<StoreMembershipItem, ApplicationError>;

    async fn set_role(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        user_id: UserId,
        role: StoreRole,
    ) -> Result<StoreMembershipItem, ApplicationError>;

    async fn leave(&self, actor: StoreActor, store_id: StoreId) -> Result<(), ApplicationError>;
}
