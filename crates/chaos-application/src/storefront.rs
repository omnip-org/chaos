use std::sync::Arc;

use chaos_domain::{
    CurrencyCode, FieldViolation, Locale,
    catalog::{CollectionHandle, ProductId},
    merchant::ApiKeyClass,
};

use crate::{
    ApplicationError,
    ports::{
        MachineActor, StorefrontCatalogProduct, StorefrontCatalogRepository, StorefrontContext,
    },
};

pub struct StorefrontProductPage {
    pub items: Vec<StorefrontCatalogProduct>,
    pub has_more: bool,
}

pub struct StorefrontCatalog {
    repository: Arc<dyn StorefrontCatalogRepository>,
}

impl StorefrontCatalog {
    pub fn new(repository: Arc<dyn StorefrontCatalogRepository>) -> Self {
        Self { repository }
    }

    pub fn context(actor: &MachineActor) -> Result<StorefrontContext, ApplicationError> {
        if actor.class != ApiKeyClass::Publishable {
            return Err(ApplicationError::Forbidden);
        }
        Ok(StorefrontContext {
            store_id: actor.store_id,
            sales_channel_id: actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_products(
        &self,
        actor: &MachineActor,
        currency: Option<&str>,
        locale: Option<&str>,
        query: Option<&str>,
        collection: Option<&str>,
        after: Option<ProductId>,
        limit: u16,
    ) -> Result<StorefrontProductPage, ApplicationError> {
        Self::context(actor)?;
        let currency = parse_currency(currency)?;
        let collection = collection
            .map(|value| CollectionHandle::parse(value.to_owned()))
            .transpose()?;
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_products(
                actor,
                currency,
                parse_locale(locale)?,
                query,
                collection.as_ref().map(CollectionHandle::as_str),
                after,
                limit + 1,
            )
            .await?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(StorefrontProductPage { items, has_more })
    }

    pub async fn get_product_by_handle(
        &self,
        actor: &MachineActor,
        currency: Option<&str>,
        locale: Option<&str>,
        handle: &str,
    ) -> Result<StorefrontCatalogProduct, ApplicationError> {
        Self::context(actor)?;
        if handle.is_empty() || handle.len() > 128 || !handle.is_ascii() {
            return Err(ApplicationError::Validation {
                violations: vec![FieldViolation {
                    field: "handle",
                    reason: "must be a valid Product handle".into(),
                }],
            });
        }
        self.repository
            .get_product_by_handle(
                actor,
                parse_currency(currency)?,
                parse_locale(locale)?,
                handle,
            )
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "product",
                id: handle.to_owned(),
            })
    }
}

fn parse_currency(value: Option<&str>) -> Result<Option<CurrencyCode>, ApplicationError> {
    value
        .map(|value| {
            CurrencyCode::parse(value).map_err(|_| ApplicationError::Validation {
                violations: vec![FieldViolation {
                    field: "currency",
                    reason: "must be an uppercase ISO 4217 currency code".into(),
                }],
            })
        })
        .transpose()
}

fn parse_locale(value: Option<&str>) -> Result<Option<Locale>, ApplicationError> {
    value.map(Locale::parse).transpose().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use chaos_domain::{
        identity::UserId,
        merchant::{ApiKeyClass, ApiKeyId, ApiKeyScope, SalesChannelId, StoreId},
    };

    use super::*;

    struct EmptyRepository;

    #[async_trait]
    impl StorefrontCatalogRepository for EmptyRepository {
        async fn list_products(
            &self,
            _actor: &MachineActor,
            _currency: Option<CurrencyCode>,
            _locale: Option<Locale>,
            _query: Option<&str>,
            _collection_handle: Option<&str>,
            _after: Option<ProductId>,
            _limit: u16,
        ) -> Result<Vec<StorefrontCatalogProduct>, ApplicationError> {
            Ok(Vec::new())
        }

        async fn get_product_by_handle(
            &self,
            _actor: &MachineActor,
            _currency: Option<CurrencyCode>,
            _locale: Option<Locale>,
            _handle: &str,
        ) -> Result<Option<StorefrontCatalogProduct>, ApplicationError> {
            Ok(None)
        }
    }

    fn actor(class: ApiKeyClass) -> MachineActor {
        MachineActor {
            api_key_id: ApiKeyId::new(),
            store_id: StoreId::new(),
            sales_channel_id: Some(SalesChannelId::new()),
            class,
            scopes: vec![ApiKeyScope::CatalogRead],
            created_by_user_id: UserId::new(),
        }
    }

    #[test]
    fn storefront_context_contains_every_resolved_boundary() {
        let actor = actor(ApiKeyClass::Publishable);
        let context = StorefrontCatalog::context(&actor).unwrap();
        assert_eq!(context.store_id, actor.store_id);
        assert_eq!(Some(context.sales_channel_id), actor.sales_channel_id);
    }

    #[tokio::test]
    async fn secret_keys_cannot_cross_the_storefront_authentication_boundary() {
        let catalog = StorefrontCatalog::new(Arc::new(EmptyRepository));
        let result = catalog
            .list_products(
                &actor(ApiKeyClass::Secret),
                None,
                None,
                None,
                None,
                None,
                20,
            )
            .await;
        assert!(matches!(result, Err(ApplicationError::Forbidden)));
    }
}
