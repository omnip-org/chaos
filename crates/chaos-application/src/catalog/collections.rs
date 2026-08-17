use std::{collections::HashSet, sync::Arc};

use chaos_domain::{
    Locale,
    catalog::{CollectionContent, CollectionHandle, CollectionId, CollectionStatus, ProductId},
    merchant::{ApiKeyScope, MerchantRole, SalesChannelId, StoreId},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    merchant::Page,
    ports::{
        AdminActor, CollectionDetail, CollectionPublicationRecord, CollectionRepository,
        CreateCollectionRecord, IdempotencyRequest, MachineActor, StorefrontCollectionItem,
    },
};

pub struct CreateCollectionInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct UpdateCollectionInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub collection_id: CollectionId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct ChangeCollectionStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub collection_id: CollectionId,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct ReplaceCollectionProductsInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub collection_id: CollectionId,
    pub product_ids: Vec<ProductId>,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct CollectionPublicationInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub collection_id: CollectionId,
    pub sales_channel_id: SalesChannelId,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct CollectionAdministration {
    repository: Arc<dyn CollectionRepository>,
}

impl CollectionAdministration {
    pub fn new(repository: Arc<dyn CollectionRepository>) -> Self {
        Self { repository }
    }

    pub async fn create(
        &self,
        input: CreateCollectionInput,
    ) -> Result<CollectionId, ApplicationError> {
        require_writer(&input.actor)?;
        let content = content(input.handle, input.title, input.description, input.metadata)?;
        self.repository
            .create(
                input.actor,
                CreateCollectionRecord {
                    id: CollectionId::new(),
                    store_id: input.store_id,
                    content,
                    created_at: input.now,
                },
                &input.idempotency,
            )
            .await
    }

    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<CollectionId>,
        limit: u16,
    ) -> Result<Page<crate::ports::CollectionListItem>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list(actor, store_id, after, limit + 1)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }

    pub async fn get(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
    ) -> Result<CollectionDetail, ApplicationError> {
        self.repository
            .get(actor, store_id, collection_id)
            .await?
            .ok_or_else(|| collection_not_found(collection_id))
    }

    pub async fn update(
        &self,
        input: UpdateCollectionInput,
    ) -> Result<CollectionId, ApplicationError> {
        require_writer(&input.actor)?;
        let content = content(input.handle, input.title, input.description, input.metadata)?;
        self.repository
            .update(
                input.actor,
                input.store_id,
                input.collection_id,
                &content,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn activate(
        &self,
        input: ChangeCollectionStatusInput,
    ) -> Result<CollectionId, ApplicationError> {
        self.status(input, CollectionStatus::Active).await
    }

    pub async fn archive(
        &self,
        input: ChangeCollectionStatusInput,
    ) -> Result<CollectionId, ApplicationError> {
        self.status(input, CollectionStatus::Archived).await
    }

    async fn status(
        &self,
        input: ChangeCollectionStatusInput,
        status: CollectionStatus,
    ) -> Result<CollectionId, ApplicationError> {
        require_writer(&input.actor)?;
        self.repository
            .set_status(
                input.actor,
                input.store_id,
                input.collection_id,
                status,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn replace_products(
        &self,
        input: ReplaceCollectionProductsInput,
    ) -> Result<CollectionId, ApplicationError> {
        require_writer(&input.actor)?;
        if input.product_ids.len() > 1_000 {
            return Err(validation(
                "product_ids",
                "must contain at most 1000 products",
            ));
        }
        let mut unique = HashSet::with_capacity(input.product_ids.len());
        if !input.product_ids.iter().all(|id| unique.insert(*id)) {
            return Err(validation("product_ids", "must not contain duplicates"));
        }
        self.repository
            .replace_products(
                input.actor,
                input.store_id,
                input.collection_id,
                &input.product_ids,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn publish(
        &self,
        input: CollectionPublicationInput,
    ) -> Result<CollectionId, ApplicationError> {
        self.publication(input, true).await
    }

    pub async fn unpublish(
        &self,
        input: CollectionPublicationInput,
    ) -> Result<CollectionId, ApplicationError> {
        self.publication(input, false).await
    }

    async fn publication(
        &self,
        input: CollectionPublicationInput,
        published: bool,
    ) -> Result<CollectionId, ApplicationError> {
        require_writer(&input.actor)?;
        self.repository
            .set_publication(
                input.actor,
                CollectionPublicationRecord {
                    store_id: input.store_id,
                    collection_id: input.collection_id,
                    sales_channel_id: input.sales_channel_id,
                    published,
                    changed_at: input.now,
                },
                &input.idempotency,
            )
            .await
    }
}

pub struct StorefrontCollections {
    repository: Arc<dyn CollectionRepository>,
}

impl StorefrontCollections {
    pub fn new(repository: Arc<dyn CollectionRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        actor: &MachineActor,
        locale: Option<&str>,
        after: Option<CollectionId>,
        limit: u16,
    ) -> Result<Page<StorefrontCollectionItem>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_storefront(actor, parse_locale(locale)?, after, limit + 1)
            .await?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(Page { items, has_more })
    }

    pub async fn get(
        &self,
        actor: &MachineActor,
        locale: Option<&str>,
        handle: &str,
    ) -> Result<StorefrontCollectionItem, ApplicationError> {
        let handle = CollectionHandle::parse(handle.to_owned())?;
        self.repository
            .get_storefront_by_handle(actor, parse_locale(locale)?, handle.as_str())
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "collection",
                id: handle.as_str().to_owned(),
            })
    }
}

fn parse_locale(value: Option<&str>) -> Result<Option<Locale>, ApplicationError> {
    value.map(Locale::parse).transpose().map_err(Into::into)
}

fn content(
    handle: String,
    title: String,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<CollectionContent, ApplicationError> {
    Ok(CollectionContent::new(
        CollectionHandle::parse(handle)?,
        title,
        description,
        crate::catalog::parse_metadata(metadata)?,
    )?)
}

fn require_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Merchant(merchant) => {
            if matches!(
                merchant.role(),
                MerchantRole::Owner
                    | MerchantRole::Administrator
                    | MerchantRole::Developer
                    | MerchantRole::Manager
            ) {
                Ok(())
            } else {
                Err(ApplicationError::Forbidden)
            }
        }
        AdminActor::Machine(machine) => {
            if machine.scopes.contains(&ApiKeyScope::CollectionsWrite) {
                Ok(())
            } else {
                Err(ApplicationError::Forbidden)
            }
        }
    }
}

fn store_not_found(id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: id.as_uuid().to_string(),
    }
}
fn collection_not_found(id: CollectionId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "collection",
        id: id.as_uuid().to_string(),
    }
}
fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}
