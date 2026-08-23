use async_trait::async_trait;
use chaos_domain::{
    identity::UserId,
    store::{SalesChannel, Store, StoreId, StoreMembership},
};

use crate::ApplicationError;

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
