use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, RegionCode,
    identity::UserId,
    store::{SalesChannel, Store, StoreId, StoreMembership, StorefrontOrigin},
};

use crate::{ApplicationError, adapters::postgres::PostgresStoreProvisioningRepository};

pub struct CreateStoreInput {
    pub user_id: UserId,
    pub name: String,
    pub region: Option<String>,
    pub currency: Option<String>,
    pub meta: Option<serde_json::Value>,
    pub origin: String,
}

#[derive(Debug)]
pub struct CreateStoreOutput {
    pub store_id: StoreId,
}

pub struct CreateStore {
    repository: Arc<PostgresStoreProvisioningRepository>,
}

impl CreateStore {
    pub fn new(repository: Arc<PostgresStoreProvisioningRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        input: CreateStoreInput,
    ) -> Result<CreateStoreOutput, ApplicationError> {
        let region = input
            .region
            .as_deref()
            .map(RegionCode::parse)
            .transpose()?
            .unwrap_or(RegionCode::US);
        let currency = input
            .currency
            .as_deref()
            .map(CurrencyCode::parse)
            .transpose()?
            .unwrap_or(CurrencyCode::USD);
        let store = Store::create(input.name, region, currency, input.meta)?;
        let origin = StorefrontOrigin::parse(input.origin)?;
        let initial_channel = SalesChannel::initial_web(store.id(), origin);
        let owner_membership = StoreMembership::owner(store.id(), input.user_id);
        let mut transaction = self.repository.begin(input.user_id).await?;

        transaction.insert_store(&store).await?;
        transaction
            .insert_owner_membership(&owner_membership)
            .await?;
        transaction.insert_initial_channel(&initial_channel).await?;
        transaction.commit().await?;

        Ok(CreateStoreOutput {
            store_id: store.id(),
        })
    }
}
