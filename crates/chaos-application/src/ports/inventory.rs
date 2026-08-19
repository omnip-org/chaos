use async_trait::async_trait;
use chaos_domain::{
    catalog::ProductVariantId,
    inventory::{
        InventoryLocation, InventoryLocationId, InventoryLocationStatus, InventoryReservation,
        InventoryReservationId, InventoryReservationStatus, StockItemId,
    },
    merchant::StoreId,
};
use time::OffsetDateTime;

use crate::{ApplicationError, merchant::StoreActor};

use super::{AdminActor, IdempotencyRequest, MachineActor};

pub struct InventoryLocationItem {
    pub id: InventoryLocationId,
    pub code: String,
    pub name: String,
    pub status: InventoryLocationStatus,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct StockItemItem {
    pub id: StockItemId,
    pub inventory_location_id: InventoryLocationId,
    pub product_variant_id: ProductVariantId,
    pub on_hand_quantity: i64,
    pub reserved_quantity: i64,
    pub available_quantity: i64,
    pub updated_at: OffsetDateTime,
}

pub struct StockAdjustment {
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

    async fn adjust_stock(
        &self,
        actor: AdminActor,
        adjustment: &StockAdjustment,
        idempotency: &IdempotencyRequest,
    ) -> Result<StockItemItem, ApplicationError>;

    async fn list_stock(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<StockItemId>,
        limit: u16,
    ) -> Result<Option<Vec<StockItemItem>>, ApplicationError>;

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
