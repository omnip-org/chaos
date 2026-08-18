use std::sync::Arc;

use chaos_domain::merchant::{MerchantRole, StoreId};
use secrecy::{ExposeSecret, SecretString};

use crate::{
    ApplicationError,
    ports::{AdminActor, ProviderSecretKind, ProviderSecretWriter, StoreAdministrationRepository},
};

use super::MerchantActor;

pub struct CreateProviderSecretInput {
    pub actor: MerchantActor,
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
        if !matches!(
            input.actor.role(),
            MerchantRole::Owner | MerchantRole::Administrator
        ) {
            return Err(ApplicationError::Forbidden);
        }
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
            .get_store(AdminActor::Merchant(input.actor), input.store_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "store",
                id: input.store_id.as_uuid().to_string(),
            })?;
        self.writer
            .create(
                input.actor.merchant_account_id(),
                input.store_id,
                input.actor.user_id(),
                input.kind,
                &input.value,
            )
            .await
    }
}
