use async_trait::async_trait;
use chaos_domain::identity::{Email, UserId};
use secrecy::SecretString;
use serde_json::Value;
use uuid::Uuid;

use crate::ApplicationError;

pub struct SessionGrant {
    pub token: SecretString,
    pub expires_in_seconds: u32,
}

pub struct CeremonyOptions {
    pub ceremony_id: Uuid,
    pub public_key: Value,
}

#[async_trait]
pub trait PasswordlessAuthentication: Send + Sync {
    async fn request_magic_link(&self, email: Email) -> Result<(), ApplicationError>;

    async fn consume_magic_link(
        &self,
        token: &SecretString,
    ) -> Result<SessionGrant, ApplicationError>;

    async fn authenticate_session(&self, token: &SecretString) -> Result<UserId, ApplicationError>;

    async fn revoke_session(&self, token: &SecretString) -> Result<(), ApplicationError>;

    async fn start_passkey_registration(
        &self,
        user_id: UserId,
    ) -> Result<CeremonyOptions, ApplicationError>;

    async fn finish_passkey_registration(
        &self,
        user_id: UserId,
        ceremony_id: Uuid,
        credential: Value,
        name: &str,
    ) -> Result<Uuid, ApplicationError>;

    async fn start_passkey_authentication(
        &self,
        email: Email,
    ) -> Result<CeremonyOptions, ApplicationError>;

    async fn finish_passkey_authentication(
        &self,
        ceremony_id: Uuid,
        credential: Value,
    ) -> Result<SessionGrant, ApplicationError>;
}
