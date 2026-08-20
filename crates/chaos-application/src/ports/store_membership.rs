use async_trait::async_trait;
use chaos_domain::{
    identity::UserId,
    merchant::{StoreId, StoreRole},
};
use time::OffsetDateTime;

use crate::{ApplicationError, merchant::StoreActor};

use super::IdempotencyRequest;

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
