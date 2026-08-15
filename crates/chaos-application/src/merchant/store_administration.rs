use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, FieldViolation, RegionCode,
    merchant::{
        MerchantRole, SalesChannel, SalesChannelCode, SalesChannelId, SalesChannelKind,
        SalesChannelStatus, Store, StoreCode, StoreId, StoreStatus,
    },
};

use crate::{
    ApplicationError,
    ports::{
        IdempotencyRequest, SalesChannelAdminItem, StoreAdminItem, StoreAdministrationRepository,
    },
};

use super::{MerchantActor, Page};

pub struct UpdateStoreInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub default_region: String,
    pub default_currency: String,
    pub idempotency: IdempotencyRequest,
}

pub struct ChangeStoreStatusInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateSalesChannelInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub kind: String,
    pub idempotency: IdempotencyRequest,
}

pub struct UpdateSalesChannelInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
    pub code: String,
    pub name: String,
    pub kind: String,
    pub idempotency: IdempotencyRequest,
}

pub struct ChangeSalesChannelStatusInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub sales_channel_id: SalesChannelId,
    pub idempotency: IdempotencyRequest,
}

pub struct StoreAdministration {
    repository: Arc<dyn StoreAdministrationRepository>,
}

impl StoreAdministration {
    pub fn new(repository: Arc<dyn StoreAdministrationRepository>) -> Self {
        Self { repository }
    }

    pub async fn get_store(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<StoreAdminItem, ApplicationError> {
        self.repository
            .get_store(actor, store_id)
            .await?
            .ok_or_else(|| store_not_found(store_id))
    }

    pub async fn update_store(&self, input: UpdateStoreInput) -> Result<StoreId, ApplicationError> {
        require_store_administrator(input.actor)?;
        let replacement = Store::create(
            input.actor.merchant_account_id(),
            StoreCode::parse(input.code)?,
            input.name,
            RegionCode::parse(&input.default_region)?,
            CurrencyCode::parse(&input.default_currency)?,
        )?;
        self.repository
            .update_store(
                input.actor,
                input.store_id,
                &replacement,
                &input.idempotency,
            )
            .await
    }

    pub async fn activate_store(
        &self,
        input: ChangeStoreStatusInput,
    ) -> Result<StoreId, ApplicationError> {
        require_store_administrator(input.actor)?;
        self.repository
            .change_store_status(
                input.actor,
                input.store_id,
                StoreStatus::Active,
                &input.idempotency,
            )
            .await
    }

    pub async fn archive_store(
        &self,
        input: ChangeStoreStatusInput,
    ) -> Result<StoreId, ApplicationError> {
        require_store_administrator(input.actor)?;
        self.repository
            .change_store_status(
                input.actor,
                input.store_id,
                StoreStatus::Archived,
                &input.idempotency,
            )
            .await
    }

    pub async fn list_sales_channels(
        &self,
        actor: MerchantActor,
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
        actor: MerchantActor,
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
        require_store_administrator(input.actor)?;
        let channel = channel(
            input.actor,
            input.store_id,
            input.code,
            input.name,
            input.kind,
        )?;
        self.repository
            .create_sales_channel(input.actor, &channel, &input.idempotency)
            .await
    }

    pub async fn update_sales_channel(
        &self,
        input: UpdateSalesChannelInput,
    ) -> Result<SalesChannelId, ApplicationError> {
        require_store_administrator(input.actor)?;
        let replacement = channel(
            input.actor,
            input.store_id,
            input.code,
            input.name,
            input.kind,
        )?;
        self.repository
            .update_sales_channel(
                input.actor,
                input.sales_channel_id,
                &replacement,
                &input.idempotency,
            )
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
        require_store_administrator(input.actor)?;
        self.repository
            .change_sales_channel_status(
                input.actor,
                input.store_id,
                input.sales_channel_id,
                status,
                &input.idempotency,
            )
            .await
    }
}

fn channel(
    actor: MerchantActor,
    store_id: StoreId,
    code: String,
    name: String,
    kind: String,
) -> Result<SalesChannel, ApplicationError> {
    let kind = SalesChannelKind::parse(&kind).ok_or_else(|| ApplicationError::Validation {
        violations: vec![FieldViolation {
            field: "kind",
            reason: "must be web, mobile, point_of_sale, marketplace, or custom".into(),
        }],
    })?;
    Ok(SalesChannel::create(
        actor.merchant_account_id(),
        store_id,
        SalesChannelCode::parse(code)?,
        name,
        kind,
    )?)
}

fn require_store_administrator(actor: MerchantActor) -> Result<(), ApplicationError> {
    match actor.role() {
        MerchantRole::Owner | MerchantRole::Administrator => Ok(()),
        MerchantRole::Developer | MerchantRole::Manager | MerchantRole::Support => {
            Err(ApplicationError::Forbidden)
        }
    }
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
