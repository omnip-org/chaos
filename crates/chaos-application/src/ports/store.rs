use async_trait::async_trait;
use chaos_domain::{
    identity::UserId,
    merchant::{MerchantAccountId, Store, StoreId},
};

use super::IdempotencyRequest;
use crate::ApplicationError;

#[async_trait]
pub trait StoreProvisioningUnitOfWork: Send + Sync {
    async fn begin(
        &self,
        user_id: UserId,
        merchant_account_id: MerchantAccountId,
    ) -> Result<Box<dyn StoreProvisioningTransaction>, ApplicationError>;
}

#[async_trait]
pub trait StoreProvisioningTransaction: Send {
    async fn can_create_store(&mut self, user_id: UserId) -> Result<bool, ApplicationError>;

    async fn reserve_store_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<StoreId>, ApplicationError>;

    async fn insert_store(&mut self, store: &Store) -> Result<(), ApplicationError>;

    async fn insert_default_currency(&mut self, store: &Store) -> Result<(), ApplicationError>;

    async fn complete_store_creation(
        &mut self,
        request: &IdempotencyRequest,
        store_id: StoreId,
    ) -> Result<(), ApplicationError>;

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError>;
}
