use std::sync::Arc;

use chaos_domain::{
    catalog::{ProductContent, ProductHandle, ProductId, ProductLifecycle},
    merchant::{ApiKeyScope, MerchantRole, SalesChannelId, StoreId},
};

use crate::{
    ApplicationError,
    ports::{AdminActor, CatalogManagementUnitOfWork, IdempotencyRequest},
};

const UPDATE_PRODUCT_OPERATION: &str = "products.update.v1";
const ACTIVATE_PRODUCT_OPERATION: &str = "products.activate.v1";
const ARCHIVE_PRODUCT_OPERATION: &str = "products.archive.v1";
const PUBLISH_PRODUCT_OPERATION: &str = "products.publish.v1";
const UNPUBLISH_PRODUCT_OPERATION: &str = "products.unpublish.v1";

pub struct UpdateProductInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub handle: String,
    pub title: String,
    pub description: String,
    pub idempotency: IdempotencyRequest,
}

pub struct ChangeProductStatusInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub idempotency: IdempotencyRequest,
}

pub struct ProductPublicationInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub sales_channel_id: SalesChannelId,
    pub idempotency: IdempotencyRequest,
}

pub struct CatalogManagement {
    unit_of_work: Arc<dyn CatalogManagementUnitOfWork>,
}

impl CatalogManagement {
    pub fn new(unit_of_work: Arc<dyn CatalogManagementUnitOfWork>) -> Self {
        Self { unit_of_work }
    }

    pub async fn update(&self, input: UpdateProductInput) -> Result<ProductId, ApplicationError> {
        require_catalog_writer(&input.actor)?;
        let content = ProductContent::new(
            ProductHandle::parse(input.handle)?,
            input.title,
            input.description,
        )?;
        let mut transaction = self
            .unit_of_work
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        if let Some(id) = transaction
            .reserve_mutation(UPDATE_PRODUCT_OPERATION, &input.idempotency)
            .await?
        {
            return Ok(id);
        }
        if !transaction.update_content(&content).await? {
            return Err(product_not_found(input.product_id));
        }
        complete(
            transaction,
            UPDATE_PRODUCT_OPERATION,
            &input.idempotency,
            input.product_id,
        )
        .await
    }

    pub async fn activate(
        &self,
        input: ChangeProductStatusInput,
    ) -> Result<ProductId, ApplicationError> {
        self.change_status(input, ACTIVATE_PRODUCT_OPERATION, true)
            .await
    }

    pub async fn archive(
        &self,
        input: ChangeProductStatusInput,
    ) -> Result<ProductId, ApplicationError> {
        self.change_status(input, ARCHIVE_PRODUCT_OPERATION, false)
            .await
    }

    pub async fn publish(
        &self,
        input: ProductPublicationInput,
    ) -> Result<ProductId, ApplicationError> {
        require_catalog_writer(&input.actor)?;
        let mut transaction = self
            .unit_of_work
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        if let Some(id) = transaction
            .reserve_mutation(PUBLISH_PRODUCT_OPERATION, &input.idempotency)
            .await?
        {
            return Ok(id);
        }
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
        complete(
            transaction,
            PUBLISH_PRODUCT_OPERATION,
            &input.idempotency,
            input.product_id,
        )
        .await
    }

    pub async fn unpublish(
        &self,
        input: ProductPublicationInput,
    ) -> Result<ProductId, ApplicationError> {
        require_catalog_writer(&input.actor)?;
        let mut transaction = self
            .unit_of_work
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        if let Some(id) = transaction
            .reserve_mutation(UNPUBLISH_PRODUCT_OPERATION, &input.idempotency)
            .await?
        {
            return Ok(id);
        }
        if transaction.load_lifecycle().await?.is_none() {
            return Err(product_not_found(input.product_id));
        }
        transaction.unpublish(input.sales_channel_id).await?;
        complete(
            transaction,
            UNPUBLISH_PRODUCT_OPERATION,
            &input.idempotency,
            input.product_id,
        )
        .await
    }

    async fn change_status(
        &self,
        input: ChangeProductStatusInput,
        operation: &'static str,
        activate: bool,
    ) -> Result<ProductId, ApplicationError> {
        require_catalog_writer(&input.actor)?;
        let mut transaction = self
            .unit_of_work
            .begin(input.actor, input.store_id, input.product_id)
            .await?;
        if let Some(id) = transaction
            .reserve_mutation(operation, &input.idempotency)
            .await?
        {
            return Ok(id);
        }
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
        complete(transaction, operation, &input.idempotency, input.product_id).await
    }
}

async fn complete(
    mut transaction: Box<dyn crate::ports::CatalogManagementTransaction>,
    operation: &'static str,
    request: &IdempotencyRequest,
    product_id: ProductId,
) -> Result<ProductId, ApplicationError> {
    transaction
        .complete_mutation(operation, request, product_id)
        .await?;
    transaction.commit().await?;
    Ok(product_id)
}

fn require_catalog_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
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
            if machine.scopes.contains(&ApiKeyScope::ProductsWrite) {
                Ok(())
            } else {
                Err(ApplicationError::Forbidden)
            }
        }
    }
}

fn product_not_found(product_id: ProductId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "product",
        id: product_id.as_uuid().to_string(),
    }
}
