use async_trait::async_trait;
use chaos_domain::{
    identity::UserId,
    merchant::{ApiKey, ApiKeyClass, ApiKeyId, ApiKeyScope, SalesChannelId, StoreId},
};
use secrecy::SecretString;
use time::OffsetDateTime;

use crate::ApplicationError;

use super::{AdminActor, IdempotencyRequest};

pub struct GeneratedApiKeyMaterial {
    pub key_identifier: String,
    pub secret_digest: [u8; 32],
    pub display_suffix: String,
    pub plaintext: SecretString,
}

pub trait ApiKeyMaterialGenerator: Send + Sync {
    fn generate(&self, class: ApiKeyClass) -> GeneratedApiKeyMaterial;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiKeyCreationStatus {
    Created,
    Replayed,
}

pub struct ApiKeyListItem {
    pub id: ApiKeyId,
    pub name: String,
    pub key_identifier: String,
    pub display_suffix: String,
    pub class: ApiKeyClass,
    pub scopes: Vec<ApiKeyScope>,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineActor {
    pub api_key_id: ApiKeyId,
    pub store_id: StoreId,
    pub sales_channel_id: Option<SalesChannelId>,
    pub class: ApiKeyClass,
    pub scopes: Vec<ApiKeyScope>,
    /// The human member who created this key. Used as the audit actor for
    /// mutations that require a real `identity.users` row (e.g. Collection
    /// events) when this key drives the mutation instead of a person.
    pub created_by_user_id: UserId,
}

#[async_trait]
pub trait ApiKeyRepository: Send + Sync {
    async fn create(
        &self,
        actor: AdminActor,
        api_key: &ApiKey,
        material: &GeneratedApiKeyMaterial,
        idempotency: &IdempotencyRequest,
    ) -> Result<ApiKeyCreationStatus, ApplicationError>;

    async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<ApiKeyId>,
        limit: u16,
    ) -> Result<Vec<ApiKeyListItem>, ApplicationError>;

    async fn revoke(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        api_key_id: ApiKeyId,
        idempotency: &IdempotencyRequest,
    ) -> Result<(), ApplicationError>;

    async fn authenticate(
        &self,
        presented_key: &SecretString,
    ) -> Result<Option<MachineActor>, ApplicationError>;
}
