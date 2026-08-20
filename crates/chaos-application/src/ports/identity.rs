use async_trait::async_trait;
use chaos_domain::identity::{Email, ExternalSubject, IdentityProvider, McpKey, McpKeyId, UserId};
use secrecy::SecretString;
use time::OffsetDateTime;

use crate::ApplicationError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedExternalIdentity {
    pub provider: IdentityProvider,
    pub subject: ExternalSubject,
    pub email: Email,
}

#[derive(Debug)]
pub struct AccessTokenGrant {
    pub user_id: UserId,
    pub token: SecretString,
    pub expires_in_seconds: u32,
}

#[async_trait]
pub trait ExternalIdentityVerifier: Send + Sync {
    async fn verify(
        &self,
        provider: IdentityProvider,
        identity_token: &SecretString,
    ) -> Result<VerifiedExternalIdentity, ApplicationError>;
}

#[async_trait]
pub trait IdentityRepository: Send + Sync {
    async fn resolve_user(
        &self,
        identity: &VerifiedExternalIdentity,
    ) -> Result<UserId, ApplicationError>;
}

pub trait AccessTokenCodec: Send + Sync {
    fn issue(&self, user_id: UserId) -> Result<AccessTokenGrant, ApplicationError>;

    fn verify(&self, token: &SecretString) -> Result<UserId, ApplicationError>;
}

#[async_trait]
pub trait IdentityAuthentication: Send + Sync {
    async fn sign_in(
        &self,
        provider: IdentityProvider,
        identity_token: &SecretString,
    ) -> Result<AccessTokenGrant, ApplicationError>;

    fn authenticate(&self, token: &SecretString) -> Result<UserId, ApplicationError>;
}

pub struct GeneratedMcpKeyMaterial {
    pub key_identifier: String,
    pub secret_digest: [u8; 32],
    pub display_suffix: String,
    pub plaintext: SecretString,
}

pub trait McpKeyMaterialGenerator: Send + Sync {
    fn generate(&self) -> GeneratedMcpKeyMaterial;
}

pub struct McpKeyListItem {
    pub id: McpKeyId,
    pub name: String,
    pub key_identifier: String,
    pub display_suffix: String,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McpPrincipal {
    pub key_id: McpKeyId,
    pub user_id: UserId,
}

#[async_trait]
pub trait McpKeyRepository: Send + Sync {
    async fn create(
        &self,
        key: &McpKey,
        material: &GeneratedMcpKeyMaterial,
    ) -> Result<(), ApplicationError>;

    async fn list(
        &self,
        user_id: UserId,
        after: Option<McpKeyId>,
        limit: u16,
    ) -> Result<Vec<McpKeyListItem>, ApplicationError>;

    async fn revoke(&self, user_id: UserId, key_id: McpKeyId) -> Result<(), ApplicationError>;

    async fn authenticate(
        &self,
        presented_key: &SecretString,
    ) -> Result<Option<McpPrincipal>, ApplicationError>;
}
