use async_trait::async_trait;
use chaos_domain::{
    identity::UserId,
    store::{PublishableKey, PublishableKeyId, PublishableKeyScope, SalesChannelId, StoreId},
};
use secrecy::SecretString;
use time::OffsetDateTime;

use crate::ApplicationError;

use super::{AdminActor, IdempotencyRequest};

pub struct GeneratedPublishableKeyMaterial {
    pub key_identifier: String,
    pub secret_digest: [u8; 32],
    pub display_suffix: String,
    pub plaintext: SecretString,
}

pub trait PublishableKeyMaterialGenerator: Send + Sync {
    fn generate(&self) -> GeneratedPublishableKeyMaterial;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishableKeyCreationStatus {
    Created,
    Replayed,
}

pub struct PublishableKeyListItem {
    pub id: PublishableKeyId,
    pub name: String,
    pub key_identifier: String,
    pub display_suffix: String,
    pub scopes: Vec<PublishableKeyScope>,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineActor {
    pub publishable_key_id: PublishableKeyId,
    pub store_id: StoreId,
    pub sales_channel_id: Option<SalesChannelId>,
    pub scopes: Vec<PublishableKeyScope>,
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
        material: &GeneratedPublishableKeyMaterial,
        idempotency: &IdempotencyRequest,
    ) -> Result<PublishableKeyCreationStatus, ApplicationError>;

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
        idempotency: &IdempotencyRequest,
    ) -> Result<(), ApplicationError>;

    async fn authenticate(
        &self,
        presented_key: &SecretString,
    ) -> Result<Option<MachineActor>, ApplicationError>;
}
