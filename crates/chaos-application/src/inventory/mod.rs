use std::sync::Arc;

use chaos_domain::{
    FieldViolation,
    catalog::ProductVariantId,
    inventory::{
        InventoryItemId, InventoryLocation, InventoryLocationCode, InventoryLocationId,
        InventoryReservation, InventoryReservationId, InventoryReservationLine,
    },
    store::StoreId,
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, InventoryAdjustment, InventoryItemView,
        InventoryLocationItem, InventoryRepository, InventoryReservationDetail,
        InventoryReservationTransition, MachineActor,
    },
    store::StoreActor,
};

pub struct CreateInventoryLocationInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub code: String,
    pub name: String,
    pub idempotency: IdempotencyRequest,
}

pub struct AdjustInventoryInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub inventory_location_id: InventoryLocationId,
    pub product_variant_id: ProductVariantId,
    pub delta_quantity: i64,
    pub note: String,
    pub idempotency: IdempotencyRequest,
}

pub struct ReserveInventoryLineInput {
    pub inventory_item_id: InventoryItemId,
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
        require_inventory_writer(&input.actor)?;
        let location = InventoryLocation::create(
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
        actor: AdminActor,
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

    pub async fn adjust_inventory_item(
        &self,
        input: AdjustInventoryInput,
    ) -> Result<InventoryItemView, ApplicationError> {
        require_inventory_writer(&input.actor)?;
        if input.delta_quantity == 0 {
            return Err(validation("delta_quantity", "must not be zero"));
        }
        if input.note.trim().is_empty() || input.note.chars().count() > 500 {
            return Err(validation("note", "must contain 1-500 characters"));
        }
        self.repository
            .adjust_inventory_item(
                input.actor,
                &InventoryAdjustment {
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

    pub async fn list_inventory_items(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<InventoryItemId>,
        limit: u16,
    ) -> Result<InventoryPage<InventoryItemView>, ApplicationError> {
        let limit = limit.clamp(1, 100);
        let mut items = self
            .repository
            .list_inventory_items(actor, store_id, after, limit + 1)
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
        require_storefront_actor(&input.actor)?;
        let lines = input
            .lines
            .into_iter()
            .map(|line| InventoryReservationLine::new(line.inventory_item_id, line.quantity))
            .collect::<Result<Vec<_>, _>>()?;
        let reservation =
            InventoryReservation::create(input.actor.store_id, input.now, input.expires_at, lines)?;
        self.repository
            .create_reservation(&input.actor, &reservation, &input.idempotency)
            .await
    }

    pub async fn release(
        &self,
        input: TransitionInventoryReservationInput,
    ) -> Result<InventoryReservationDetail, ApplicationError> {
        require_storefront_actor(&input.actor)?;
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
        require_storefront_actor(&input.actor)?;
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
        actor: StoreActor,
        store_id: StoreId,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<u16, ApplicationError> {
        require_inventory_writer(&AdminActor::Store(actor))?;
        self.repository
            .expire_due_reservations(actor, store_id, now, limit.clamp(1, 500))
            .await
    }
}

fn require_inventory_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}

fn require_storefront_actor(actor: &MachineActor) -> Result<(), ApplicationError> {
    if actor.sales_channel_id.is_some() {
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
