use std::sync::Arc;

use chaos_domain::{FieldViolation, catalog::ProductVariantId, store::StoreId};

use crate::{
    ApplicationError,
    ports::{AdminActor, InventoryAdjustment, InventoryRepository, VariantInventoryView},
};

pub struct AdjustInventoryInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_variant_id: ProductVariantId,
    pub delta_quantity: i64,
    pub note: String,
}

pub struct InventoryManagement {
    repository: Arc<dyn InventoryRepository>,
}

pub struct InventoryPage<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}

impl InventoryManagement {
    pub fn new(repository: Arc<dyn InventoryRepository>) -> Self {
        Self { repository }
    }

    pub async fn adjust_variant_inventory(
        &self,
        input: AdjustInventoryInput,
    ) -> Result<VariantInventoryView, ApplicationError> {
        require_inventory_writer(&input.actor)?;
        if input.delta_quantity == 0 {
            return Err(validation("delta_quantity", "must not be zero"));
        }
        if input.note.trim().is_empty() || input.note.chars().count() > 500 {
            return Err(validation("note", "must contain 1-500 characters"));
        }
        self.repository
            .adjust_variant_inventory(
                input.actor,
                &InventoryAdjustment {
                    store_id: input.store_id,
                    product_variant_id: input.product_variant_id,
                    delta_quantity: input.delta_quantity,
                    note: input.note.trim().into(),
                },
            )
            .await
    }

    pub async fn list_variant_inventory(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<ProductVariantId>,
        limit: u16,
    ) -> Result<InventoryPage<VariantInventoryView>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_variant_inventory(actor, store_id, after, limit + 1)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(InventoryPage { items, has_more })
    }
}

fn require_inventory_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}
