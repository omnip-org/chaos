use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    merchant::{ApiKey, ApiKeyClass, ApiKeyId, ApiKeyScope, StoreId, StoreRole},
};
use secrecy::SecretString;

use crate::{
    ApplicationError,
    ports::{
        AdminActor, ApiKeyCreationStatus, ApiKeyListItem, ApiKeyMaterialGenerator,
        ApiKeyRepository, IdempotencyRequest, MachineActor,
    },
};

use super::Page;

pub struct CreateApiKeyInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub name: String,
    pub class: String,
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
        authorize_api_key_management(&input.actor, ApiKeyScope::ApiKeysWrite)?;
        let class = parse_class(&input.class)?;
        let scopes = input
            .scopes
            .iter()
            .map(|scope| parse_scope(scope))
            .collect::<Result<Vec<_>, _>>()?;
        let api_key = ApiKey::issue(input.store_id, input.name, class, scopes)?;
        let material = self.generator.generate(class);
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
        actor: AdminActor,
        store_id: StoreId,
        after: Option<ApiKeyId>,
        limit: u16,
    ) -> Result<Page<ApiKeyListItem>, ApplicationError> {
        authorize_api_key_management(&actor, ApiKeyScope::ApiKeysRead)?;
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
        actor: AdminActor,
        store_id: StoreId,
        api_key_id: ApiKeyId,
        idempotency: IdempotencyRequest,
    ) -> Result<(), ApplicationError> {
        authorize_api_key_management(&actor, ApiKeyScope::ApiKeysWrite)?;
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

fn authorize_api_key_management(
    actor: &AdminActor,
    required_scope: ApiKeyScope,
) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(store_actor) => match store_actor.role() {
            StoreRole::Owner => Ok(()),
            StoreRole::Member => Err(ApplicationError::Forbidden),
        },
        AdminActor::Machine(machine) => {
            if machine.scopes.contains(&required_scope) {
                Ok(())
            } else {
                Err(ApplicationError::Forbidden)
            }
        }
    }
}

fn parse_class(value: &str) -> Result<ApiKeyClass, ApplicationError> {
    ApiKeyClass::parse(value).ok_or_else(|| invalid_enum("class", "must be publishable or secret"))
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
    use chaos_domain::identity::UserId;

    use super::*;
    use crate::merchant::StoreActor;

    fn actor(role: StoreRole) -> AdminActor {
        AdminActor::Store(StoreActor::new(UserId::new(), StoreId::new(), role))
    }

    #[test]
    fn only_credential_administrators_can_manage_api_keys() {
        assert!(
            authorize_api_key_management(&actor(StoreRole::Owner), ApiKeyScope::ApiKeysWrite)
                .is_ok()
        );
        assert!(matches!(
            authorize_api_key_management(&actor(StoreRole::Member), ApiKeyScope::ApiKeysWrite),
            Err(ApplicationError::Forbidden)
        ));
    }

    #[test]
    fn machine_actor_needs_the_required_scope() {
        use chaos_domain::merchant::{ApiKeyClass, ApiKeyId};

        let machine = |scopes: Vec<ApiKeyScope>| {
            AdminActor::Machine(MachineActor {
                api_key_id: ApiKeyId::new(),
                store_id: StoreId::new(),
                sales_channel_id: None,
                class: ApiKeyClass::Secret,
                scopes,
                created_by_user_id: UserId::new(),
            })
        };

        assert!(
            authorize_api_key_management(
                &machine(vec![ApiKeyScope::ApiKeysWrite]),
                ApiKeyScope::ApiKeysWrite
            )
            .is_ok()
        );
        assert!(matches!(
            authorize_api_key_management(
                &machine(vec![ApiKeyScope::ApiKeysRead]),
                ApiKeyScope::ApiKeysWrite
            ),
            Err(ApplicationError::Forbidden)
        ));
    }
}
