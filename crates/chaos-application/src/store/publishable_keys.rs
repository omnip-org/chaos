use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    store::{PublishableKey, PublishableKeyId, PublishableKeyScope, StoreId, StoreRole},
};
use secrecy::SecretString;

use crate::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, MachineActor, PublishableKeyCreationStatus,
        PublishableKeyListItem, PublishableKeyMaterialGenerator, PublishableKeyRepository,
    },
};

use super::Page;

pub struct CreatePublishableKeyInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub name: String,
    pub scopes: Vec<String>,
    pub idempotency: IdempotencyRequest,
}

pub struct CreatePublishableKeyOutput {
    pub publishable_key: PublishableKey,
    pub key_identifier: String,
    pub display_suffix: String,
    pub plaintext: SecretString,
}

pub struct PublishableKeyManagement {
    repository: Arc<dyn PublishableKeyRepository>,
    generator: Arc<dyn PublishableKeyMaterialGenerator>,
}

impl PublishableKeyManagement {
    pub fn new(
        repository: Arc<dyn PublishableKeyRepository>,
        generator: Arc<dyn PublishableKeyMaterialGenerator>,
    ) -> Self {
        Self {
            repository,
            generator,
        }
    }

    pub async fn create(
        &self,
        input: CreatePublishableKeyInput,
    ) -> Result<CreatePublishableKeyOutput, ApplicationError> {
        authorize_publishable_key_management(&input.actor)?;
        let scopes = input
            .scopes
            .iter()
            .map(|scope| parse_scope(scope))
            .collect::<Result<Vec<_>, _>>()?;
        let publishable_key = PublishableKey::issue(input.store_id, input.name, scopes)?;
        let material = self.generator.generate();
        let status = self
            .repository
            .create(input.actor, &publishable_key, &material, &input.idempotency)
            .await?;
        if status == PublishableKeyCreationStatus::Replayed {
            return Err(ApplicationError::Conflict {
                code: "publishable_key_secret_already_issued",
                message: "the Publishable Key was already created and its secret cannot be shown again",
            });
        }

        Ok(CreatePublishableKeyOutput {
            publishable_key,
            key_identifier: material.key_identifier,
            display_suffix: material.display_suffix,
            plaintext: material.plaintext,
        })
    }

    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<PublishableKeyId>,
        limit: u16,
    ) -> Result<Page<PublishableKeyListItem>, ApplicationError> {
        authorize_publishable_key_management(&actor)?;
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
        publishable_key_id: PublishableKeyId,
        idempotency: IdempotencyRequest,
    ) -> Result<(), ApplicationError> {
        authorize_publishable_key_management(&actor)?;
        self.repository
            .revoke(actor, store_id, publishable_key_id, &idempotency)
            .await
    }
}

pub struct PublishableKeyAuthentication {
    repository: Arc<dyn PublishableKeyRepository>,
}

impl PublishableKeyAuthentication {
    pub fn new(repository: Arc<dyn PublishableKeyRepository>) -> Self {
        Self { repository }
    }

    pub async fn authenticate(
        &self,
        presented_key: &SecretString,
        required_scopes: &[PublishableKeyScope],
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

fn authorize_publishable_key_management(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(store_actor) => match store_actor.role() {
            StoreRole::Owner => Ok(()),
            StoreRole::Member => Err(ApplicationError::Forbidden),
        },
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}

fn parse_scope(value: &str) -> Result<PublishableKeyScope, ApplicationError> {
    PublishableKeyScope::parse(value)
        .ok_or_else(|| invalid_enum("scopes", "contains an unknown scope"))
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
    use crate::store::StoreActor;

    fn actor(role: StoreRole) -> AdminActor {
        AdminActor::Store(StoreActor::new(UserId::new(), StoreId::new(), role))
    }

    #[test]
    fn only_credential_administrators_can_manage_publishable_keys() {
        assert!(authorize_publishable_key_management(&actor(StoreRole::Owner)).is_ok());
        assert!(matches!(
            authorize_publishable_key_management(&actor(StoreRole::Member)),
            Err(ApplicationError::Forbidden)
        ));
    }
}
