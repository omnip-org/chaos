use std::sync::Arc;

use async_trait::async_trait;
use chaos_domain::identity::{IdentityProvider, McpKey, McpKeyId, UserId};
use secrecy::SecretString;

use crate::{
    ApplicationError,
    ports::{
        AccessTokenCodec, AccessTokenGrant, ExternalIdentityVerifier, IdentityAuthentication,
        IdentityRepository, McpKeyListItem, McpKeyMaterialGenerator, McpKeyRepository,
        McpPrincipal,
    },
};

pub struct IdentityService {
    verifier: Arc<dyn ExternalIdentityVerifier>,
    repository: Arc<dyn IdentityRepository>,
    tokens: Arc<dyn AccessTokenCodec>,
}

pub struct CreateMcpKeyOutput {
    pub key: McpKey,
    pub key_identifier: String,
    pub display_suffix: String,
    pub plaintext: SecretString,
}

pub struct McpKeyPage {
    pub items: Vec<McpKeyListItem>,
    pub has_more: bool,
}

pub struct McpKeyManagement {
    repository: Arc<dyn McpKeyRepository>,
    generator: Arc<dyn McpKeyMaterialGenerator>,
}

impl McpKeyManagement {
    pub fn new(
        repository: Arc<dyn McpKeyRepository>,
        generator: Arc<dyn McpKeyMaterialGenerator>,
    ) -> Self {
        Self {
            repository,
            generator,
        }
    }

    pub async fn create(
        &self,
        user_id: UserId,
        name: String,
    ) -> Result<CreateMcpKeyOutput, ApplicationError> {
        let key = McpKey::issue(user_id, name).map_err(ApplicationError::from)?;
        let material = self.generator.generate();
        self.repository.create(&key, &material).await?;
        Ok(CreateMcpKeyOutput {
            key,
            key_identifier: material.key_identifier,
            display_suffix: material.display_suffix,
            plaintext: material.plaintext,
        })
    }

    pub async fn list(
        &self,
        user_id: UserId,
        after: Option<McpKeyId>,
        limit: u16,
    ) -> Result<McpKeyPage, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self.repository.list(user_id, after, limit + 1).await?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(McpKeyPage { items, has_more })
    }

    pub async fn revoke(&self, user_id: UserId, key_id: McpKeyId) -> Result<(), ApplicationError> {
        self.repository.revoke(user_id, key_id).await
    }
}

pub struct McpKeyAuthentication {
    repository: Arc<dyn McpKeyRepository>,
}

impl McpKeyAuthentication {
    pub fn new(repository: Arc<dyn McpKeyRepository>) -> Self {
        Self { repository }
    }

    pub async fn authenticate(
        &self,
        presented_key: &SecretString,
    ) -> Result<McpPrincipal, ApplicationError> {
        self.repository
            .authenticate(presented_key)
            .await?
            .ok_or(ApplicationError::Unauthorized)
    }
}

impl IdentityService {
    pub fn new(
        verifier: Arc<dyn ExternalIdentityVerifier>,
        repository: Arc<dyn IdentityRepository>,
        tokens: Arc<dyn AccessTokenCodec>,
    ) -> Self {
        Self {
            verifier,
            repository,
            tokens,
        }
    }
}

#[async_trait]
impl IdentityAuthentication for IdentityService {
    async fn sign_in(
        &self,
        provider: IdentityProvider,
        identity_token: &SecretString,
    ) -> Result<AccessTokenGrant, ApplicationError> {
        let identity = self.verifier.verify(provider, identity_token).await?;
        let user_id = self.repository.resolve_user(&identity).await?;
        self.tokens.issue(user_id)
    }

    fn authenticate(&self, token: &SecretString) -> Result<UserId, ApplicationError> {
        self.tokens.verify(token)
    }
}

#[cfg(test)]
mod tests {
    use chaos_domain::identity::{Email, ExternalSubject};

    use super::*;
    use crate::ports::VerifiedExternalIdentity;

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

    struct FixedTokens(UserId);

    impl AccessTokenCodec for FixedTokens {
        fn issue(&self, user_id: UserId) -> Result<AccessTokenGrant, ApplicationError> {
            assert_eq!(user_id, self.0);
            Ok(AccessTokenGrant {
                user_id,
                token: SecretString::from("access-token"),
                expires_in_seconds: 900,
            })
        }

        fn verify(&self, _token: &SecretString) -> Result<UserId, ApplicationError> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn sign_in_verifies_resolves_and_issues_in_order() {
        let user_id = UserId::new();
        let service = IdentityService::new(
            Arc::new(FixedVerifier),
            Arc::new(FixedRepository(user_id)),
            Arc::new(FixedTokens(user_id)),
        );
        let grant = service
            .sign_in(
                IdentityProvider::Google,
                &SecretString::from("provider-token"),
            )
            .await
            .unwrap();
        assert_eq!(grant.expires_in_seconds, 900);
        assert_eq!(
            service
                .authenticate(&SecretString::from("access-token"))
                .unwrap(),
            user_id
        );
    }
}
