use async_trait::async_trait;
use chaos_domain::{
    catalog::ProductVariantId,
    inventory::{
        InventoryItemId, InventoryLocation, InventoryLocationId, InventoryReservation,
        InventoryReservationId, InventoryReservationStatus,
    },
    store::StoreId,
};
use time::OffsetDateTime;

use crate::{ApplicationError, store::StoreActor};

use super::{AdminActor, IdempotencyRequest, MachineActor};

pub struct InventoryLocationItem {
    pub id: InventoryLocationId,
    pub code: String,
    pub name: String,
    pub archived_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct InventoryItemView {
    pub id: InventoryItemId,
    pub inventory_location_id: InventoryLocationId,
    pub product_variant_id: ProductVariantId,
    pub on_hand_quantity: i64,
    pub reserved_quantity: i64,
    pub available_quantity: i64,
    pub updated_at: OffsetDateTime,
}

pub struct InventoryAdjustment {
    pub store_id: StoreId,
    pub inventory_location_id: InventoryLocationId,
    pub product_variant_id: ProductVariantId,
    pub delta_quantity: i64,
    pub note: String,
}

pub struct InventoryReservationDetail {
    pub id: InventoryReservationId,
    pub status: InventoryReservationStatus,
    pub expires_at: OffsetDateTime,
    pub closed_at: Option<OffsetDateTime>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryReservationTransition {
    Release,
    Consume,
}

#[async_trait]
pub trait InventoryRepository: Send + Sync {
    async fn create_location(
        &self,
        actor: AdminActor,
        location: &InventoryLocation,
        idempotency: &IdempotencyRequest,
    ) -> Result<InventoryLocationId, ApplicationError>;

    async fn list_locations(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<InventoryLocationId>,
        limit: u16,
    ) -> Result<Option<Vec<InventoryLocationItem>>, ApplicationError>;

    async fn adjust_inventory_item(
        &self,
        actor: AdminActor,
        adjustment: &InventoryAdjustment,
        idempotency: &IdempotencyRequest,
    ) -> Result<InventoryItemView, ApplicationError>;

    async fn list_inventory_items(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<InventoryItemId>,
        limit: u16,
    ) -> Result<Option<Vec<InventoryItemView>>, ApplicationError>;

    async fn create_reservation(
        &self,
        actor: &MachineActor,
        reservation: &InventoryReservation,
        idempotency: &IdempotencyRequest,
    ) -> Result<InventoryReservationId, ApplicationError>;

    async fn transition_reservation(
        &self,
        actor: &MachineActor,
        reservation_id: InventoryReservationId,
        transition: InventoryReservationTransition,
        now: OffsetDateTime,
        idempotency: &IdempotencyRequest,
    ) -> Result<InventoryReservationDetail, ApplicationError>;

    async fn expire_due_reservations(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<u16, ApplicationError>;
}
