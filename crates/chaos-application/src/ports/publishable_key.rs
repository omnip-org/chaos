use async_trait::async_trait;
use chaos_domain::{
    identity::UserId,
    store::{PublishableKey, PublishableKeyId, SalesChannelId, StoreId},
};
use time::OffsetDateTime;

use crate::ApplicationError;

use super::AdminActor;

pub struct GeneratedPublishableKey {
    pub public_key: String,
}

pub trait PublishableKeyGenerator: Send + Sync {
    fn generate(&self) -> GeneratedPublishableKey;
}

pub struct PublishableKeyListItem {
    pub id: PublishableKeyId,
    pub name: String,
    pub public_key: String,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineActor {
    pub publishable_key_id: PublishableKeyId,
    pub store_id: StoreId,
    pub sales_channel_id: Option<SalesChannelId>,
    /// The human member who created this key. Used as the audit actor for
    /// mutations that require a real `identity.users` row (e.g. Collection
    /// events) when this key drives the mutation instead of a person.
    pub created_by_user_id: UserId,
}

#[async_trait]
pub trait PublishableKeyRepository: Send + Sync {
    async fn create(
        &self,
        actor: AdminActor,
        publishable_key: &PublishableKey,
        generated_key: &GeneratedPublishableKey,
    ) -> Result<(PublishableKeyId, String), ApplicationError>;

    async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<PublishableKeyId>,
        limit: u16,
    ) -> Result<Vec<PublishableKeyListItem>, ApplicationError>;

    async fn revoke(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        publishable_key_id: PublishableKeyId,
    ) -> Result<(), ApplicationError>;

    async fn authenticate(
        &self,
        presented_key: &str,
    ) -> Result<Option<MachineActor>, ApplicationError>;
}
