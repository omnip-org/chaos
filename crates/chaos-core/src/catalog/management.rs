use std::sync::Arc;

use chaos_domain::{
    catalog::{
        ProductContent, ProductHandle, ProductId, ProductLifecycle, ProductVariantContent,
        ProductVariantId, Sku,
    },
    store::{SalesChannelId, StoreId},
};

use crate::{
    ApplicationError, adapters::postgres::PostgresCatalogManagementRepository,
    catalog::parse_metadata, contracts::AdminActor,
};

pub struct UpdateProductInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
    pub expected_revision: Option<i64>,
}

pub struct UpdateProductVariantInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub title: String,
    pub sku: Option<String>,
    pub track_inventory: bool,
    pub metadata: Option<serde_json::Value>,
    pub expected_revision: Option<i64>,
}

pub struct PatchProductInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub handle: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// None preserves metadata; Some(None) clears it; Some(Some(value)) replaces it.
    pub metadata: Option<Option<serde_json::Value>>,
    pub expected_revision: Option<i64>,
}

pub struct PatchProductVariantInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub title: Option<String>,
    /// None preserves the SKU; Some(None) clears it.
    pub sku: Option<Option<String>>,
    pub track_inventory: Option<bool>,
    /// None preserves metadata; Some(None) clears it; Some(Some(value)) replaces it.
    pub metadata: Option<Option<serde_json::Value>>,
    pub expected_revision: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductMutationOutput {
    pub product_id: ProductId,
    pub revision: i64,
}

pub struct ChangeProductStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub expected_revision: Option<i64>,
}

pub struct ProductPublicationInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub sales_channel_id: SalesChannelId,
    pub expected_revision: Option<i64>,
}

pub struct CatalogManagement {
    repository: Arc<PostgresCatalogManagementRepository>,
}

impl CatalogManagement {
    pub fn new(repository: Arc<PostgresCatalogManagementRepository>) -> Self {
        Self { repository }
    }

    pub async fn update(
        &self,
        input: UpdateProductInput,
    ) -> Result<ProductMutationOutput, ApplicationError> {
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
        let current = transaction
            .load_product_content()
            .await?
            .ok_or_else(|| product_not_found(input.product_id))?;
        ensure_revision(current.revision, input.expected_revision)?;
        if !transaction.update_content(&content).await? {
            return Err(product_not_found(input.product_id));
        }
        let revision = transaction.product_revision().await?;
        transaction.commit().await?;
        Ok(ProductMutationOutput {
            product_id: input.product_id,
            revision,
        })
    }

    pub async fn update_variant(
        &self,
        input: UpdateProductVariantInput,
    ) -> Result<ProductMutationOutput, ApplicationError> {
        input.actor.require_human()?;
        let content = ProductVariantContent::new(
            input.title,
            input.sku.map(Sku::parse).transpose()?,
            input.track_inventory,
            parse_metadata(input.metadata)?,
        )?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        let current = transaction
            .load_variant_content(input.product_variant_id)
            .await?
            .ok_or_else(|| product_variant_not_found(input.product_variant_id))?;
        ensure_revision(current.revision, input.expected_revision)?;
        if !transaction
            .update_variant_content(input.product_variant_id, &content)
            .await?
        {
            return Err(product_variant_not_found(input.product_variant_id));
        }
        let revision = transaction.product_revision().await?;
        transaction.commit().await?;
        Ok(ProductMutationOutput {
            product_id: input.product_id,
            revision,
        })
    }

    pub async fn patch(
        &self,
        input: PatchProductInput,
    ) -> Result<ProductMutationOutput, ApplicationError> {
        input.actor.require_human()?;
        if input.handle.is_none()
            && input.title.is_none()
            && input.description.is_none()
            && input.metadata.is_none()
        {
            return Err(no_patch_fields("Product"));
        }
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        let current = transaction
            .load_product_content()
            .await?
            .ok_or_else(|| product_not_found(input.product_id))?;
        ensure_revision(current.revision, input.expected_revision)?;
        let content = ProductContent::new(
            ProductHandle::parse(input.handle.unwrap_or(current.handle))?,
            input.title.unwrap_or(current.title),
            input.description.unwrap_or(current.description),
            parse_metadata(match input.metadata {
                Some(metadata) => metadata,
                None => current.meta,
            })?,
        )?;
        transaction.update_content(&content).await?;
        let revision = transaction.product_revision().await?;
        transaction.commit().await?;
        Ok(ProductMutationOutput {
            product_id: input.product_id,
            revision,
        })
    }

    pub async fn patch_variant(
        &self,
        input: PatchProductVariantInput,
    ) -> Result<ProductMutationOutput, ApplicationError> {
        input.actor.require_human()?;
        if input.title.is_none()
            && input.sku.is_none()
            && input.track_inventory.is_none()
            && input.metadata.is_none()
        {
            return Err(no_patch_fields("Product Variant"));
        }
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        let current = transaction
            .load_variant_content(input.product_variant_id)
            .await?
            .ok_or_else(|| product_variant_not_found(input.product_variant_id))?;
        ensure_revision(current.revision, input.expected_revision)?;
        let content = ProductVariantContent::new(
            input.title.unwrap_or(current.title),
            match input.sku {
                Some(sku) => sku.map(Sku::parse).transpose()?,
                None => current.sku.map(Sku::parse).transpose()?,
            },
            input.track_inventory.unwrap_or(current.track_inventory),
            parse_metadata(match input.metadata {
                Some(metadata) => metadata,
                None => current.meta,
            })?,
        )?;
        if !transaction
            .update_variant_content(input.product_variant_id, &content)
            .await?
        {
            return Err(product_variant_not_found(input.product_variant_id));
        }
        let revision = transaction.product_revision().await?;
        transaction.commit().await?;
        Ok(ProductMutationOutput {
            product_id: input.product_id,
            revision,
        })
    }

    pub async fn activate(
        &self,
        input: ChangeProductStatusInput,
    ) -> Result<ProductMutationOutput, ApplicationError> {
        self.change_status(input, true).await
    }

    pub async fn archive(
        &self,
        input: ChangeProductStatusInput,
    ) -> Result<ProductMutationOutput, ApplicationError> {
        self.change_status(input, false).await
    }

    pub async fn publish(
        &self,
        input: ProductPublicationInput,
    ) -> Result<ProductMutationOutput, ApplicationError> {
        input.actor.require_human()?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        let snapshot = transaction
            .load_lifecycle()
            .await?
            .ok_or_else(|| product_not_found(input.product_id))?;
        ensure_revision(snapshot.revision, input.expected_revision)?;
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
        let revision = transaction.product_revision().await?;
        transaction.commit().await?;
        Ok(ProductMutationOutput {
            product_id: input.product_id,
            revision,
        })
    }

    pub async fn unpublish(
        &self,
        input: ProductPublicationInput,
    ) -> Result<ProductMutationOutput, ApplicationError> {
        input.actor.require_human()?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        let snapshot = transaction
            .load_lifecycle()
            .await?
            .ok_or_else(|| product_not_found(input.product_id))?;
        ensure_revision(snapshot.revision, input.expected_revision)?;
        transaction.unpublish(input.sales_channel_id).await?;
        let revision = transaction.product_revision().await?;
        transaction.commit().await?;
        Ok(ProductMutationOutput {
            product_id: input.product_id,
            revision,
        })
    }

    async fn change_status(
        &self,
        input: ChangeProductStatusInput,
        activate: bool,
    ) -> Result<ProductMutationOutput, ApplicationError> {
        input.actor.require_human()?;
        let mut transaction = self
            .repository
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        let snapshot = transaction
            .load_lifecycle()
            .await?
            .ok_or_else(|| product_not_found(input.product_id))?;
        ensure_revision(snapshot.revision, input.expected_revision)?;
        let mut lifecycle =
            ProductLifecycle::from_snapshot(snapshot.status, snapshot.variant_count);
        if activate {
            lifecycle.activate()?;
        } else {
            lifecycle.archive();
        }
        transaction.set_status(lifecycle.status()).await?;
        let revision = transaction.product_revision().await?;
        transaction.commit().await?;
        Ok(ProductMutationOutput {
            product_id: input.product_id,
            revision,
        })
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

fn ensure_revision(current: i64, expected: Option<i64>) -> Result<(), ApplicationError> {
    if expected.is_some_and(|expected| expected != current) {
        return Err(ApplicationError::Conflict {
            code: "product_revision_mismatch",
            message: "the Product changed; refresh the workspace and retry with its current revision",
        });
    }
    Ok(())
}

fn no_patch_fields(resource: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "patch",
            reason: format!("at least one {resource} field must be provided"),
        }],
    }
}
