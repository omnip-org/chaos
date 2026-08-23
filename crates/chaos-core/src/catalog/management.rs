use std::sync::Arc;

use chaos_domain::{
    catalog::{
        ProductContent, ProductHandle, ProductId, ProductLifecycle, ProductVariantContent,
        ProductVariantId, Sku,
    },
    store::{SalesChannelId, StoreId},
};

use crate::{
    ApplicationError, catalog::parse_metadata, ports::AdminActor,
    repositories::PostgresCatalogManagementRepository,
};

pub struct UpdateProductInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
}

pub struct UpdateProductVariantInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub title: String,
    pub sku: Option<String>,
    pub requires_shipping: bool,
    pub track_inventory: bool,
    pub metadata: Option<serde_json::Value>,
}

pub struct ChangeProductStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
}

pub struct ProductPublicationInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub sales_channel_id: SalesChannelId,
}

pub struct CatalogManagement {
    repository: Arc<PostgresCatalogManagementRepository>,
}

impl CatalogManagement {
    pub fn new(repository: Arc<PostgresCatalogManagementRepository>) -> Self {
        Self { repository }
    }

    pub async fn update(&self, input: UpdateProductInput) -> Result<ProductId, ApplicationError> {
        input.actor.require_human()?;
        let content = ProductContent::new(
            ProductHandle::parse(input.handle)?,
            input.title,
            input.description,
            parse_metadata(input.metadata)?,
        )?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        if !transaction.update_content(&content).await? {
            return Err(product_not_found(input.product_id));
        }
        transaction.commit().await.map(|()| input.product_id)
    }

    pub async fn update_variant(
        &self,
        input: UpdateProductVariantInput,
    ) -> Result<ProductVariantId, ApplicationError> {
        input.actor.require_human()?;
        let content = ProductVariantContent::new(
            input.title,
            input.sku.map(Sku::parse).transpose()?,
            input.requires_shipping,
            input.track_inventory,
            parse_metadata(input.metadata)?,
        )?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        if !transaction
            .update_variant_content(input.product_variant_id, &content)
            .await?
        {
            return Err(product_variant_not_found(input.product_variant_id));
        }
        transaction
            .commit()
            .await
            .map(|()| input.product_variant_id)
    }

    pub async fn activate(
        &self,
        input: ChangeProductStatusInput,
    ) -> Result<ProductId, ApplicationError> {
        self.change_status(input, true).await
    }

    pub async fn archive(
        &self,
        input: ChangeProductStatusInput,
    ) -> Result<ProductId, ApplicationError> {
        self.change_status(input, false).await
    }

    pub async fn publish(
        &self,
        input: ProductPublicationInput,
    ) -> Result<ProductId, ApplicationError> {
        input.actor.require_human()?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        let snapshot = transaction
            .load_lifecycle()
            .await?
            .ok_or_else(|| product_not_found(input.product_id))?;
        ProductLifecycle::from_snapshot(snapshot.status, snapshot.variant_count)
            .require_publishable()?;
        if !transaction
            .active_channel_exists(input.sales_channel_id)
            .await?
        {
            return Err(ApplicationError::NotFound {
                resource: "sales_channel",
                id: input.sales_channel_id.as_uuid().to_string(),
            });
        }
        transaction.publish(input.sales_channel_id).await?;
        transaction.commit().await.map(|()| input.product_id)
    }

    pub async fn unpublish(
        &self,
        input: ProductPublicationInput,
    ) -> Result<ProductId, ApplicationError> {
        input.actor.require_human()?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        if transaction.load_lifecycle().await?.is_none() {
            return Err(product_not_found(input.product_id));
        }
        transaction.unpublish(input.sales_channel_id).await?;
        transaction.commit().await.map(|()| input.product_id)
    }

    async fn change_status(
        &self,
        input: ChangeProductStatusInput,
        activate: bool,
    ) -> Result<ProductId, ApplicationError> {
        input.actor.require_human()?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        let snapshot = transaction
            .load_lifecycle()
            .await?
            .ok_or_else(|| product_not_found(input.product_id))?;
        let mut lifecycle =
            ProductLifecycle::from_snapshot(snapshot.status, snapshot.variant_count);
        if activate {
            lifecycle.activate()?;
        } else {
            lifecycle.archive();
        }
        transaction.set_status(lifecycle.status()).await?;
        transaction.commit().await.map(|()| input.product_id)
    }
}

fn product_not_found(product_id: ProductId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "product",
        id: product_id.as_uuid().to_string(),
    }
}

fn product_variant_not_found(variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "product_variant",
        id: variant_id.as_uuid().to_string(),
    }
}
