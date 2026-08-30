use std::sync::Arc;

use async_trait::async_trait;
use chaos_domain::identity::{IdentityProvider, UserId};
use secrecy::SecretString;

use crate::{
    ApplicationError,
    contracts::{ExternalIdentityVerifier, IdentityAuthentication, IdentityRepository},
};

pub struct IdentityService {
    verifier: Arc<dyn ExternalIdentityVerifier>,
    repository: Arc<dyn IdentityRepository>,
}

impl IdentityService {
    pub fn new(
        verifier: Arc<dyn ExternalIdentityVerifier>,
        repository: Arc<dyn IdentityRepository>,
    ) -> Self {
        Self {
            verifier,
            repository,
        }
    }
}

#[async_trait]
impl IdentityAuthentication for IdentityService {
    async fn sign_in(
        &self,
        provider: IdentityProvider,
        identity_token: &SecretString,
    ) -> Result<UserId, ApplicationError> {
        let identity = self.verifier.verify(provider, identity_token).await?;
        self.repository.resolve_user(&identity).await
    }
}

#[cfg(test)]
mod tests {
    use chaos_domain::identity::{Email, ExternalSubject};

    use super::*;
    use crate::contracts::VerifiedExternalIdentity;

    struct FixedVerifier;

    #[async_trait]
    impl ExternalIdentityVerifier for FixedVerifier {
        async fn verify(
            &self,
            provider: IdentityProvider,
            _identity_token: &SecretString,
        ) -> Result<VerifiedExternalIdentity, ApplicationError> {
            Ok(VerifiedExternalIdentity {
                provider,
                subject: ExternalSubject::parse("provider-subject").unwrap(),
                email: Email::parse("person@example.com").unwrap(),
            })
        }
    }

    struct FixedRepository(UserId);

    #[async_trait]
    impl IdentityRepository for FixedRepository {
        async fn resolve_user(
            &self,
            _identity: &VerifiedExternalIdentity,
        ) -> Result<UserId, ApplicationError> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn sign_in_verifies_and_resolves_in_order() {
        let user_id = UserId::new();
        let service =
            IdentityService::new(Arc::new(FixedVerifier), Arc::new(FixedRepository(user_id)));
        let resolved_user_id = service
            .sign_in(
                IdentityProvider::Google,
                &SecretString::from("provider-token"),
            )
            .await
            .unwrap();
        assert_eq!(resolved_user_id, user_id);
    }
}
