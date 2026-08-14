use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    merchant::{ApiKey, ApiKeyClass, ApiKeyId, ApiKeyMode, ApiKeyScope, MerchantRole, StoreId},
};
use secrecy::SecretString;

use crate::{
    ApplicationError,
    ports::{
        ApiKeyCreationStatus, ApiKeyListItem, ApiKeyMaterialGenerator, ApiKeyRepository,
        IdempotencyRequest, MachineActor,
    },
};

use super::{MerchantActor, Page};

pub struct CreateApiKeyInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub name: String,
    pub class: String,
    pub mode: String,
    pub scopes: Vec<String>,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateApiKeyOutput {
    pub api_key: ApiKey,
    pub key_identifier: String,
    pub display_suffix: String,
    pub plaintext: SecretString,
}

pub struct ApiKeyManagement {
    repository: Arc<dyn ApiKeyRepository>,
    generator: Arc<dyn ApiKeyMaterialGenerator>,
}

impl ApiKeyManagement {
    pub fn new(
        repository: Arc<dyn ApiKeyRepository>,
        generator: Arc<dyn ApiKeyMaterialGenerator>,
    ) -> Self {
        Self {
            repository,
            generator,
        }
    }

    pub async fn create(
        &self,
        input: CreateApiKeyInput,
    ) -> Result<CreateApiKeyOutput, ApplicationError> {
        authorize_api_key_management(input.actor)?;
        let class = parse_class(&input.class)?;
        let mode = parse_mode(&input.mode)?;
        let scopes = input
            .scopes
            .iter()
            .map(|scope| parse_scope(scope))
            .collect::<Result<Vec<_>, _>>()?;
        let api_key = ApiKey::issue(
            input.actor.merchant_account_id(),
            input.store_id,
            input.name,
            class,
            mode,
            scopes,
        )?;
        let material = self.generator.generate(class, mode);
        let status = self
            .repository
            .create(input.actor, &api_key, &material, &input.idempotency)
            .await?;
        if status == ApiKeyCreationStatus::Replayed {
            return Err(ApplicationError::Conflict {
                code: "api_key_secret_already_issued",
                message: "the API key was already created and its secret cannot be shown again",
            });
        }

        Ok(CreateApiKeyOutput {
            api_key,
            key_identifier: material.key_identifier,
            display_suffix: material.display_suffix,
            plaintext: material.plaintext,
        })
    }

    pub async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<ApiKeyId>,
        limit: u16,
    ) -> Result<Page<ApiKeyListItem>, ApplicationError> {
        authorize_api_key_management(actor)?;
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list(actor, store_id, after, limit + 1)
            .await?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }

    pub async fn revoke(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        api_key_id: ApiKeyId,
        idempotency: IdempotencyRequest,
    ) -> Result<(), ApplicationError> {
        authorize_api_key_management(actor)?;
        self.repository
            .revoke(actor, store_id, api_key_id, &idempotency)
            .await
    }
}

pub struct ApiKeyAuthentication {
    repository: Arc<dyn ApiKeyRepository>,
}

impl ApiKeyAuthentication {
    pub fn new(repository: Arc<dyn ApiKeyRepository>) -> Self {
        Self { repository }
    }

    pub async fn authenticate(
        &self,
        presented_key: &SecretString,
        required_scopes: &[ApiKeyScope],
    ) -> Result<MachineActor, ApplicationError> {
        let actor = self
            .repository
            .authenticate(presented_key)
            .await?
            .ok_or(ApplicationError::Unauthorized)?;
        if required_scopes
            .iter()
            .any(|required_scope| !actor.scopes.contains(required_scope))
        {
            return Err(ApplicationError::Forbidden);
        }
        Ok(actor)
    }
}

fn authorize_api_key_management(actor: MerchantActor) -> Result<(), ApplicationError> {
    match actor.role() {
        MerchantRole::Owner | MerchantRole::Administrator | MerchantRole::Developer => Ok(()),
        MerchantRole::Manager | MerchantRole::Support => Err(ApplicationError::Forbidden),
    }
}

fn parse_class(value: &str) -> Result<ApiKeyClass, ApplicationError> {
    ApiKeyClass::parse(value).ok_or_else(|| invalid_enum("class", "must be publishable or secret"))
}

fn parse_mode(value: &str) -> Result<ApiKeyMode, ApplicationError> {
    ApiKeyMode::parse(value).ok_or_else(|| invalid_enum("mode", "must be test or live"))
}

fn parse_scope(value: &str) -> Result<ApiKeyScope, ApplicationError> {
    ApiKeyScope::parse(value).ok_or_else(|| invalid_enum("scopes", "contains an unknown scope"))
}

fn invalid_enum(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use chaos_domain::{
        identity::UserId,
        merchant::{MerchantAccountId, MerchantRole},
    };

    use super::*;

    fn actor(role: MerchantRole) -> MerchantActor {
        MerchantActor::new(UserId::new(), MerchantAccountId::new(), role)
    }

    #[test]
    fn only_credential_administrators_can_manage_api_keys() {
        for role in [
            MerchantRole::Owner,
            MerchantRole::Administrator,
            MerchantRole::Developer,
        ] {
            assert!(authorize_api_key_management(actor(role)).is_ok());
        }
        for role in [MerchantRole::Manager, MerchantRole::Support] {
            assert!(matches!(
                authorize_api_key_management(actor(role)),
                Err(ApplicationError::Forbidden)
            ));
        }
    }
}
