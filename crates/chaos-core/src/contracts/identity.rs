use async_trait::async_trait;
use chaos_domain::identity::{Email, ExternalSubject, IdentityProvider, UserId};
use secrecy::SecretString;

use crate::ApplicationError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedExternalIdentity {
    pub provider: IdentityProvider,
    pub subject: ExternalSubject,
    pub email: Email,
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

#[async_trait]
pub trait IdentityAuthentication: Send + Sync {
    async fn sign_in(
        &self,
        provider: IdentityProvider,
        identity_token: &SecretString,
    ) -> Result<UserId, ApplicationError>;
}
