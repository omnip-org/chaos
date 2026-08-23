use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode, RegionCode,
    identity::UserId,
    store::{SalesChannel, Store, StoreCode, StoreId, StoreMembership, StoreRole, StoreStatus},
};
use time::OffsetDateTime;

use crate::{ApplicationError, store::StoreActor};

pub struct IdempotencyRequest {
    pub key: String,
    pub request_fingerprint: [u8; 32],
}

#[async_trait]
pub trait StoreProvisioningUnitOfWork: Send + Sync {
    async fn begin(
        &self,
        user_id: UserId,
    ) -> Result<Box<dyn StoreProvisioningTransaction>, ApplicationError>;
}

#[async_trait]
pub trait StoreProvisioningTransaction: Send {
    async fn reserve_store_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<StoreId>, ApplicationError>;

    async fn insert_store(&mut self, store: &Store) -> Result<(), ApplicationError>;

    async fn insert_owner_membership(
        &mut self,
        membership: &StoreMembership,
    ) -> Result<(), ApplicationError>;

    async fn insert_default_currency(&mut self, store: &Store) -> Result<(), ApplicationError>;

    async fn insert_default_sales_channel(
        &mut self,
        channel: &SalesChannel,
    ) -> Result<(), ApplicationError>;

    async fn complete_store_creation(
        &mut self,
        request: &IdempotencyRequest,
        store_id: StoreId,
    ) -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}

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
        request: &IdempotencyRequest,
    ) -> Result<StoreMembershipItem, ApplicationError>;

    async fn set_role(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        user_id: UserId,
        role: StoreRole,
        request: &IdempotencyRequest,
    ) -> Result<StoreMembershipItem, ApplicationError>;

    async fn leave(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        request: &IdempotencyRequest,
    ) -> Result<(), ApplicationError>;
}
