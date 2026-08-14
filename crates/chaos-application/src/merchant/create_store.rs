use std::sync::Arc;

use chaos_domain::{
    CurrencyCode,
    identity::UserId,
    merchant::{MerchantAccountId, Store, StoreCode, StoreId},
};

use crate::{
    ApplicationError,
    ports::{IdempotencyRequest, StoreProvisioningUnitOfWork},
};

pub struct CreateStoreInput {
    pub user_id: UserId,
    pub merchant_account_id: MerchantAccountId,
    pub code: String,
    pub name: String,
    pub default_currency: String,
    pub idempotency: IdempotencyRequest,
}

#[derive(Debug)]
pub struct CreateStoreOutput {
    pub store_id: StoreId,
}

pub struct CreateStore {
    unit_of_work: Arc<dyn StoreProvisioningUnitOfWork>,
}

impl CreateStore {
    pub fn new(unit_of_work: Arc<dyn StoreProvisioningUnitOfWork>) -> Self {
        Self { unit_of_work }
    }

    pub async fn execute(
        &self,
        input: CreateStoreInput,
    ) -> Result<CreateStoreOutput, ApplicationError> {
        let store = Store::create(
            input.merchant_account_id,
            StoreCode::parse(input.code)?,
            input.name,
            CurrencyCode::parse(&input.default_currency)?,
        )?;
        let mut transaction = self
            .unit_of_work
            .begin(input.user_id, input.merchant_account_id)
            .await?;

        if !transaction.can_create_store(input.user_id).await? {
            return Err(ApplicationError::Forbidden);
        }
        if let Some(store_id) = transaction
            .reserve_store_creation(&input.idempotency)
            .await?
        {
            return Ok(CreateStoreOutput { store_id });
        }

        transaction.insert_store(&store).await?;
        transaction.insert_default_currency(&store).await?;
        transaction
            .complete_store_creation(&input.idempotency, store.id())
            .await?;
        transaction.commit().await?;

        Ok(CreateStoreOutput {
            store_id: store.id(),
        })
    }
}
