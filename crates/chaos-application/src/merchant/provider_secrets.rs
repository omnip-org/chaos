use std::sync::Arc;

use chaos_domain::merchant::{ApiKeyScope, StoreId, StoreRole};
use secrecy::{ExposeSecret, SecretString};

use crate::{
    ApplicationError,
    ports::{AdminActor, ProviderSecretKind, ProviderSecretWriter, StoreAdministrationRepository},
};

pub struct CreateProviderSecretInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub kind: ProviderSecretKind,
    pub value: SecretString,
}

pub struct ProviderSecretManagement {
    stores: Arc<dyn StoreAdministrationRepository>,
    writer: Arc<dyn ProviderSecretWriter>,
}

impl ProviderSecretManagement {
    pub fn new(
        stores: Arc<dyn StoreAdministrationRepository>,
        writer: Arc<dyn ProviderSecretWriter>,
    ) -> Self {
        Self { stores, writer }
    }

    pub async fn create(
        &self,
        input: CreateProviderSecretInput,
    ) -> Result<String, ApplicationError> {
        require_provider_secret_writer(&input.actor)?;
        let value = input.value.expose_secret();
        if value.trim().is_empty() || value.len() > 16 * 1024 || value.chars().any(char::is_control)
        {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "value",
                    reason: "must contain 1-16384 control-free bytes".into(),
                }],
            });
        }
        self.stores
            .get_store(input.actor.clone(), input.store_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "store",
                id: input.store_id.as_uuid().to_string(),
            })?;
        self.writer
            .create(
                input.store_id,
                input.actor.audit_user_id(),
                input.kind,
                &input.value,
            )
            .await
    }
}

fn require_provider_secret_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(store_actor) => {
            if matches!(store_actor.role(), StoreRole::Owner) {
                Ok(())
            } else {
                Err(ApplicationError::Forbidden)
            }
        }
        AdminActor::Machine(machine) => {
            if machine.scopes.contains(&ApiKeyScope::ProviderSecretsWrite) {
                Ok(())
            } else {
                Err(ApplicationError::Forbidden)
            }
        }
    }
}
