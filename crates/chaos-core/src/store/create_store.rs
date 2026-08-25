use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, RegionCode,
    identity::UserId,
    store::{SalesChannel, Store, StoreCode, StoreId, StoreMembership, StorefrontOrigin},
};

use crate::{ApplicationError, adapters::postgres::PostgresStoreProvisioningRepository};

pub struct CreateStoreInput {
    pub user_id: UserId,
    pub code: String,
    pub name: String,
    pub region: Option<String>,
    pub currency: Option<String>,
    pub meta: Option<serde_json::Value>,
    pub storefront_origin: String,
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
        let store = Store::create(
            StoreCode::parse(input.code)?,
            input.name,
            region,
            currency,
            input.meta,
        )?;
        let storefront_origin = StorefrontOrigin::parse(input.storefront_origin)?;
        let default_sales_channel = SalesChannel::default_web(store.id(), storefront_origin);
        let owner_membership = StoreMembership::owner(store.id(), input.user_id);
        let mut transaction = self.repository.begin(input.user_id).await?;

        transaction.insert_store(&store).await?;
        transaction
            .insert_owner_membership(&owner_membership)
            .await?;
        transaction
            .insert_default_sales_channel(&default_sales_channel)
            .await?;
        transaction.commit().await?;

        Ok(CreateStoreOutput {
            store_id: store.id(),
        })
    }
}
