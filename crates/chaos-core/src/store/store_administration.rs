use std::sync::Arc;

use chaos_domain::{
    RegionCode,
    store::{
        SalesChannel, SalesChannelId, SalesChannelStatus, Store, StoreId, StoreStatus,
        StorefrontOrigin,
    },
};

use crate::{
    ApplicationError,
    adapters::postgres::PostgresStoreAdministrationRepository,
    contracts::{AdminActor, SalesChannelAdminItem, ShippingCountryAdminItem, StoreAdminItem},
};

use super::Page;

pub struct UpdateStoreInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub name: String,
    pub region: String,
    pub meta: Option<serde_json::Value>,
}

pub struct ChangeStoreStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
}

pub struct CreateSalesChannelInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub name: String,
    pub origin: String,
}

pub struct UpdateSalesChannelInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub channel_id: SalesChannelId,
    pub name: String,
    pub origin: String,
}

pub struct ChangeSalesChannelStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub channel_id: SalesChannelId,
}

pub struct SetShippingCountryInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub country_code: String,
    pub enabled: bool,
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
        Store::validate_name(&input.name)?;
        let region = RegionCode::parse(&input.region)?;
        self.repository
            .update_store(input.actor, input.store_id, &input.name, region, input.meta)
            .await
    }

    pub async fn list_shipping_countries(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingCountryAdminItem>, ApplicationError> {
        self.repository
            .list_shipping_countries(actor, store_id)
            .await?
            .ok_or_else(|| store_not_found(store_id))
    }

    pub async fn set_shipping_country(
        &self,
        input: SetShippingCountryInput,
    ) -> Result<ShippingCountryAdminItem, ApplicationError> {
        input.actor.require_owner()?;
        let country_code = parse_shipping_country_code(input.country_code)?;
        self.repository
            .set_shipping_country(input.actor, input.store_id, &country_code, input.enabled)
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
        channel_id: SalesChannelId,
    ) -> Result<SalesChannelAdminItem, ApplicationError> {
        self.repository
            .get_sales_channel(actor, store_id, channel_id)
            .await?
            .ok_or_else(|| channel_not_found(channel_id))
    }

    pub async fn create_sales_channel(
        &self,
        input: CreateSalesChannelInput,
    ) -> Result<SalesChannelId, ApplicationError> {
        input.actor.require_owner()?;
        let channel = channel(input.store_id, input.name, input.origin)?;
        self.repository
            .create_sales_channel(input.actor, &channel)
            .await
    }

    pub async fn update_sales_channel(
        &self,
        input: UpdateSalesChannelInput,
    ) -> Result<SalesChannelId, ApplicationError> {
        input.actor.require_owner()?;
        let replacement = channel(input.store_id, input.name, input.origin)?;
        self.repository
            .update_sales_channel(input.actor, input.channel_id, &replacement)
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
            .change_sales_channel_status(input.actor, input.store_id, input.channel_id, status)
            .await
    }
}

fn channel(
    store_id: StoreId,
    name: String,
    origin: String,
) -> Result<SalesChannel, ApplicationError> {
    Ok(SalesChannel::create(
        store_id,
        name,
        StorefrontOrigin::parse(origin)?,
    )?)
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

fn channel_not_found(channel_id: SalesChannelId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "channel",
        id: channel_id.as_uuid().to_string(),
    }
}

fn parse_shipping_country_code(value: String) -> Result<String, ApplicationError> {
    let value = value.trim().to_ascii_uppercase();
    if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "country_code",
                reason: "must be a two-letter ISO 3166-1 alpha-2 code".into(),
            }],
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::parse_shipping_country_code;
    use crate::ApplicationError;

    #[test]
    fn normalizes_shipping_country_codes() {
        assert_eq!(parse_shipping_country_code(" ca ".into()).unwrap(), "CA");
    }

    #[test]
    fn rejects_invalid_shipping_country_codes() {
        assert!(matches!(
            parse_shipping_country_code("CAN".into()),
            Err(ApplicationError::Validation { .. })
        ));
        assert!(matches!(
            parse_shipping_country_code("C1".into()),
            Err(ApplicationError::Validation { .. })
        ));
    }
}
