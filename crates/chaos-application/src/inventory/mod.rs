use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    catalog::ProductVariantId,
    inventory::{
        InventoryLocation, InventoryLocationCode, InventoryLocationId, InventoryReservation,
        InventoryReservationId, InventoryReservationLine, StockItemId,
    },
    merchant::{ApiKeyScope, MerchantRole, StoreId},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        IdempotencyRequest, InventoryLocationItem, InventoryRepository, InventoryReservationDetail,
        InventoryReservationTransition, MachineActor, StockAdjustment, StockItemItem,
    },
};

pub struct CreateInventoryLocationInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub idempotency: IdempotencyRequest,
}

pub struct AdjustStockInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub inventory_location_id: InventoryLocationId,
    pub product_variant_id: ProductVariantId,
    pub delta_quantity: i64,
    pub note: String,
    pub idempotency: IdempotencyRequest,
}

pub struct ReserveInventoryLineInput {
    pub stock_item_id: StockItemId,
    pub product_variant_id: ProductVariantId,
    pub quantity: i64,
}

pub struct ReserveInventoryInput {
    pub actor: MachineActor,
    pub now: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub lines: Vec<ReserveInventoryLineInput>,
    pub idempotency: IdempotencyRequest,
}

pub struct TransitionInventoryReservationInput {
    pub actor: MachineActor,
    pub reservation_id: InventoryReservationId,
    pub now: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
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

    pub async fn create_location(
        &self,
        input: CreateInventoryLocationInput,
    ) -> Result<InventoryLocationId, ApplicationError> {
        require_inventory_writer(input.actor)?;
        let location = InventoryLocation::create(
            input.actor.merchant_account_id(),
            input.store_id,
            InventoryLocationCode::parse(input.code)?,
            input.name,
        )?;
        self.repository
            .create_location(input.actor, &location, &input.idempotency)
            .await
    }

    pub async fn list_locations(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<InventoryLocationId>,
        limit: u16,
    ) -> Result<InventoryPage<InventoryLocationItem>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_locations(actor, store_id, after, limit + 1)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(InventoryPage { items, has_more })
    }

    pub async fn adjust_stock(
        &self,
        input: AdjustStockInput,
    ) -> Result<StockItemItem, ApplicationError> {
        require_inventory_writer(input.actor)?;
        if input.delta_quantity == 0 {
            return Err(validation("delta_quantity", "must not be zero"));
        }
        if input.note.trim().is_empty() || input.note.chars().count() > 500 {
            return Err(validation("note", "must contain 1-500 characters"));
        }
        self.repository
            .adjust_stock(
                input.actor,
                &StockAdjustment {
                    store_id: input.store_id,
                    inventory_location_id: input.inventory_location_id,
                    product_variant_id: input.product_variant_id,
                    delta_quantity: input.delta_quantity,
                    note: input.note.trim().into(),
                },
                &input.idempotency,
            )
            .await
    }

    pub async fn list_stock(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<StockItemId>,
        limit: u16,
    ) -> Result<InventoryPage<StockItemItem>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_stock(actor, store_id, after, limit + 1)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        let has_more = items.len() > usize::from(limit);
        if has_more {
            items.pop();
        }
        Ok(InventoryPage { items, has_more })
    }

    pub async fn reserve(
        &self,
        input: ReserveInventoryInput,
    ) -> Result<InventoryReservationId, ApplicationError> {
        require_machine_scope(&input.actor, ApiKeyScope::CheckoutWrite)?;
        if input.actor.sales_channel_id.is_none() {
            return Err(validation(
                "sales_channel_id",
                "machine credential must resolve a Sales Channel",
            ));
        }
        let lines = input
            .lines
            .into_iter()
            .map(|line| {
                InventoryReservationLine::new(
                    line.stock_item_id,
                    line.product_variant_id,
                    line.quantity,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let reservation = InventoryReservation::create(
            input.actor.merchant_account_id,
            input.actor.store_id,
            input.now,
            input.expires_at,
            lines,
        )?;
        self.repository
            .create_reservation(&input.actor, &reservation, &input.idempotency)
            .await
    }

    pub async fn release(
        &self,
        input: TransitionInventoryReservationInput,
    ) -> Result<InventoryReservationDetail, ApplicationError> {
        require_machine_scope(&input.actor, ApiKeyScope::CheckoutWrite)?;
        self.repository
            .transition_reservation(
                &input.actor,
                input.reservation_id,
                InventoryReservationTransition::Release,
                input.now,
                &input.idempotency,
            )
            .await
    }

    pub async fn consume(
        &self,
        input: TransitionInventoryReservationInput,
    ) -> Result<InventoryReservationDetail, ApplicationError> {
        require_machine_scope(&input.actor, ApiKeyScope::CheckoutWrite)?;
        self.repository
            .transition_reservation(
                &input.actor,
                input.reservation_id,
                InventoryReservationTransition::Consume,
                input.now,
                &input.idempotency,
            )
            .await
    }

    pub async fn expire_due(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<u16, ApplicationError> {
        require_inventory_writer(actor)?;
        self.repository
            .expire_due_reservations(actor, store_id, now, limit.clamp(1, 500))
            .await
    }
}

fn require_inventory_writer(actor: MerchantActor) -> Result<(), ApplicationError> {
    match actor.role() {
        MerchantRole::Owner
        | MerchantRole::Administrator
        | MerchantRole::Developer
        | MerchantRole::Manager => Ok(()),
        MerchantRole::Support => Err(ApplicationError::Forbidden),
    }
}

fn require_machine_scope(
    actor: &MachineActor,
    required: ApiKeyScope,
) -> Result<(), ApplicationError> {
    if actor.scopes.contains(&required) {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
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
