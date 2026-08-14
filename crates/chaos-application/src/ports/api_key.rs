use async_trait::async_trait;
use chaos_domain::merchant::{
    ApiKey, ApiKeyClass, ApiKeyId, ApiKeyMode, ApiKeyScope, MerchantAccountId, StoreId,
};
use secrecy::SecretString;
use time::OffsetDateTime;

use crate::{ApplicationError, merchant::MerchantActor};

use super::IdempotencyRequest;

pub struct GeneratedApiKeyMaterial {
    pub key_identifier: String,
    pub secret_digest: [u8; 32],
    pub display_suffix: String,
    pub plaintext: SecretString,
}

pub trait ApiKeyMaterialGenerator: Send + Sync {
    fn generate(&self, class: ApiKeyClass, mode: ApiKeyMode) -> GeneratedApiKeyMaterial;
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
    pub mode: ApiKeyMode,
    pub scopes: Vec<ApiKeyScope>,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineActor {
    pub api_key_id: ApiKeyId,
    pub merchant_account_id: MerchantAccountId,
    pub store_id: StoreId,
    pub class: ApiKeyClass,
    pub mode: ApiKeyMode,
    pub scopes: Vec<ApiKeyScope>,
}

#[async_trait]
pub trait ApiKeyRepository: Send + Sync {
    async fn create(
        &self,
        actor: MerchantActor,
        api_key: &ApiKey,
        material: &GeneratedApiKeyMaterial,
        idempotency: &IdempotencyRequest,
    ) -> Result<ApiKeyCreationStatus, ApplicationError>;

    async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<ApiKeyId>,
        limit: u16,
    ) -> Result<Vec<ApiKeyListItem>, ApplicationError>;

    async fn revoke(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        api_key_id: ApiKeyId,
        idempotency: &IdempotencyRequest,
    ) -> Result<(), ApplicationError>;

    async fn authenticate(
        &self,
        presented_key: &SecretString,
    ) -> Result<Option<MachineActor>, ApplicationError>;
}
