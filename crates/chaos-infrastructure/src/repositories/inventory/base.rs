// Inventory repository imports, row shapes, constructor, and shared constants.

use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, InventoryAdjustment, InventoryItemView,
        InventoryLocationItem, InventoryRepository, InventoryReservationDetail,
        InventoryReservationTransition, MachineActor,
    },
    store::StoreActor,
};
use chaos_domain::{
    catalog::ProductVariantId,
    inventory::{
        InventoryItemId, InventoryLocation, InventoryLocationId, InventoryReservation,
        InventoryBalance, InventoryReservationId, InventoryReservationStatus,
    },
    store::StoreId,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_LOCATION_OPERATION: &str = "inventory_locations.create.v1";
const ADJUST_INVENTORY_OPERATION: &str = "inventory_items.adjust.v1";
const CREATE_RESERVATION_OPERATION: &str = "inventory_reservations.create.v1";
const RELEASE_RESERVATION_OPERATION: &str = "inventory_reservations.release.v1";
const CONSUME_RESERVATION_OPERATION: &str = "inventory_reservations.consume.v1";

type LocationRow = (
    Uuid,
    String,
    String,
    Option<OffsetDateTime>,
    OffsetDateTime,
    OffsetDateTime,
);
type InventoryItemRow = (Uuid, Uuid, Uuid, i64, i64, OffsetDateTime);
type ReservationRow = (String, OffsetDateTime, Option<OffsetDateTime>, Uuid);
type LockedInventoryItemRow = (Uuid, i64, i64);

#[derive(Clone)]
pub struct PostgresInventoryRepository {
    pool: PgPool,
}

impl PostgresInventoryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_for_store_actor(
        &self,
        actor: StoreActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(
            &mut transaction,
            Some(actor.user_id().as_uuid()),
            actor.store_id().as_uuid(),
        )
        .await?;
        Ok(transaction)
    }

    async fn begin_for_machine(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(&mut transaction, None, actor.store_id.as_uuid()).await?;
        Ok(transaction)
    }

    async fn begin_for_admin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(
            &mut transaction,
            Some(actor.audit_user_id().as_uuid()),
            actor.store_id().as_uuid(),
        )
        .await?;
        Ok(transaction)
    }
}
