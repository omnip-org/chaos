use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, RegionCode,
    store::{
        SalesChannel, SalesChannelCode, SalesChannelId, SalesChannelStatus, Store, StoreCode,
        StoreId, StoreStatus,
    },
};

use crate::{
    ApplicationError,
    ports::{AdminActor, SalesChannelAdminItem, StoreAdminItem},
    repositories::PostgresStoreAdministrationRepository,
};

use super::Page;

pub struct UpdateStoreInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub region: String,
    pub currency: String,
    pub meta: Option<serde_json::Value>,
}

pub struct ChangeStoreStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
}

pub struct CreateSalesChannelInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
}

pub struct UpdateSalesChannelInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
    pub code: String,
    pub name: String,
}

pub struct ChangeSalesChannelStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
}

pub struct StoreAdministration {
    repository: Arc<PostgresStoreAdministrationRepository>,
}

impl StoreAdministration {
    pub fn new(repository: Arc<PostgresStoreAdministrationRepository>) -> Self {
        Self { repository }
    }

    pub async fn get_store(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<StoreAdminItem, ApplicationError> {
        self.repository
            .get_store(actor, store_id)
            .await?
            .ok_or_else(|| store_not_found(store_id))
    }

    pub async fn update_store(&self, input: UpdateStoreInput) -> Result<StoreId, ApplicationError> {
        input.actor.require_owner()?;
        let replacement = Store::create(
            StoreCode::parse(input.code)?,
            input.name,
            RegionCode::parse(&input.region)?,
            CurrencyCode::parse(&input.currency)?,
            input.meta,
        )?;
        self.repository
            .update_store(input.actor, input.store_id, &replacement)
            .await
    }

    pub async fn activate_store(
        &self,
        input: ChangeStoreStatusInput,
    ) -> Result<StoreId, ApplicationError> {
        input.actor.require_owner()?;
        self.repository
            .change_store_status(input.actor, input.store_id, StoreStatus::Active)
            .await
    }

    pub async fn archive_store(
        &self,
        input: ChangeStoreStatusInput,
    ) -> Result<StoreId, ApplicationError> {
        input.actor.require_owner()?;
        self.repository
            .change_store_status(input.actor, input.store_id, StoreStatus::Inactive)
            .await
    }

    pub async fn list_sales_channels(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<SalesChannelId>,
        limit: u16,
    ) -> Result<Page<SalesChannelAdminItem>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_sales_channels(actor, store_id, after, limit + 1)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }

    pub async fn get_sales_channel(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
    ) -> Result<SalesChannelAdminItem, ApplicationError> {
        self.repository
            .get_sales_channel(actor, store_id, sales_channel_id)
            .await?
            .ok_or_else(|| channel_not_found(sales_channel_id))
    }

    pub async fn create_sales_channel(
        &self,
        input: CreateSalesChannelInput,
    ) -> Result<SalesChannelId, ApplicationError> {
        input.actor.require_owner()?;
        let channel = channel(input.store_id, input.code, input.name)?;
        self.repository
            .create_sales_channel(input.actor, &channel)
            .await
    }

    pub async fn update_sales_channel(
        &self,
        input: UpdateSalesChannelInput,
    ) -> Result<SalesChannelId, ApplicationError> {
        input.actor.require_owner()?;
        let replacement = channel(input.store_id, input.code, input.name)?;
        self.repository
            .update_sales_channel(input.actor, input.sales_channel_id, &replacement)
            .await
    }

    pub async fn activate_sales_channel(
        &self,
        input: ChangeSalesChannelStatusInput,
    ) -> Result<SalesChannelId, ApplicationError> {
        self.change_channel_status(input, SalesChannelStatus::Active)
            .await
    }

    pub async fn archive_sales_channel(
        &self,
        input: ChangeSalesChannelStatusInput,
    ) -> Result<SalesChannelId, ApplicationError> {
        self.change_channel_status(input, SalesChannelStatus::Archived)
            .await
    }

    async fn change_channel_status(
        &self,
        input: ChangeSalesChannelStatusInput,
        status: SalesChannelStatus,
    ) -> Result<SalesChannelId, ApplicationError> {
        input.actor.require_owner()?;
        self.repository
            .change_sales_channel_status(
                input.actor,
                input.store_id,
                input.sales_channel_id,
                status,
            )
            .await
    }
}

fn channel(
    store_id: StoreId,
    code: String,
    name: String,
) -> Result<SalesChannel, ApplicationError> {
    Ok(SalesChannel::create(
        store_id,
        SalesChannelCode::parse(code)?,
        name,
    )?)
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

fn channel_not_found(sales_channel_id: SalesChannelId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "sales_channel",
        id: sales_channel_id.as_uuid().to_string(),
    }
}
