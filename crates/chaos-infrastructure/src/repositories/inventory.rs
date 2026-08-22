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
        InventoryReservationId, InventoryReservationStatus, StockBalance,
    },
    store::StoreId,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_LOCATION_OPERATION: &str = "inventory_locations.create.v1";
const ADJUST_STOCK_OPERATION: &str = "inventory_items.adjust.v1";
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

#[async_trait]
impl InventoryRepository for PostgresInventoryRepository {
    async fn create_location(
        &self,
        actor: AdminActor,
        location: &InventoryLocation,
        request: &IdempotencyRequest,
    ) -> Result<InventoryLocationId, ApplicationError> {
        let store_id = actor.store_id();
        let mut transaction = self.begin_for_admin(&actor).await?;
        if let Some(snapshot) = reserve_idempotency(
            &mut transaction,
            store_id.as_uuid(),
            CREATE_LOCATION_OPERATION,
            request,
        )
        .await?
        {
            return replay_id(&snapshot).map(InventoryLocationId::from_uuid);
        }
        require_store(&mut transaction, location.store_id()).await?;
        sqlx::query(
            "INSERT INTO commerce.inventory_locations \
             (id, store_id, code, name) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(location.id().as_uuid())
        .bind(location.store_id().as_uuid())
        .bind(location.code().as_str())
        .bind(location.name())
        .execute(&mut *transaction)
        .await
        .map_err(map_location_write_error)?;
        complete_id(
            &mut transaction,
            store_id.as_uuid(),
            CREATE_LOCATION_OPERATION,
            request,
            location.id().as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(location.id())
    }

    async fn list_locations(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<InventoryLocationId>,
        limit: u16,
    ) -> Result<Option<Vec<InventoryLocationItem>>, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        if !store_exists(&mut transaction, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, LocationRow>(
            "SELECT id, code::text, name, archived_at, created_at, updated_at \
             FROM commerce.inventory_locations \
             WHERE store_id = $1 \
               AND ($2::uuid IS NULL OR id > $2) ORDER BY id ASC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(InventoryLocationId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(location_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn adjust_stock(
        &self,
        actor: AdminActor,
        adjustment: &InventoryAdjustment,
        request: &IdempotencyRequest,
    ) -> Result<InventoryItemView, ApplicationError> {
        let audit_user_id = actor.audit_user_id();
        let mut transaction = self.begin_for_admin(&actor).await?;
        if let Some(snapshot) = reserve_idempotency(
            &mut transaction,
            adjustment.store_id.as_uuid(),
            ADJUST_STOCK_OPERATION,
            request,
        )
        .await?
        {
            return replay_inventory_item(&snapshot);
        }
        require_store(&mut transaction, adjustment.store_id).await?;
        sqlx::query(
            "INSERT INTO commerce.inventory_items \
             (id, store_id, inventory_location_id, product_variant_id) \
             SELECT $1, $2, location.id, variant.id \
             FROM commerce.inventory_locations AS location \
             INNER JOIN commerce.product_variants AS variant \
               ON variant.store_id = location.store_id \
              AND variant.id = $4 AND variant.track_inventory \
             WHERE location.store_id = $2 \
               AND location.id = $3 AND location.archived_at IS NULL \
             ON CONFLICT (store_id, inventory_location_id, product_variant_id) \
             DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(adjustment.store_id.as_uuid())
        .bind(adjustment.inventory_location_id.as_uuid())
        .bind(adjustment.product_variant_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let locked = lock_inventory_item_by_location_variant(
            &mut transaction,
            adjustment.store_id,
            adjustment.inventory_location_id,
            adjustment.product_variant_id,
        )
        .await?
        .ok_or_else(invalid_inventory_selection)?;
        let balance = StockBalance::new(locked.1, locked.2)?.adjust(adjustment.delta_quantity)?;
        let updated_at = update_inventory_balance(&mut transaction, locked.0, balance).await?;
        insert_inventory_transaction(
            &mut transaction,
            adjustment.store_id,
            locked.0,
            None,
            None,
            adjustment.delta_quantity,
            0,
            balance,
            Some(&adjustment.note),
            Some(audit_user_id.as_uuid()),
        )
        .await?;
        let item = InventoryItemView {
            id: InventoryItemId::from_uuid(locked.0),
            inventory_location_id: adjustment.inventory_location_id,
            product_variant_id: adjustment.product_variant_id,
            on_hand_quantity: balance.on_hand(),
            reserved_quantity: balance.reserved(),
            available_quantity: balance.available(),
            updated_at,
        };
        complete_snapshot(
            &mut transaction,
            adjustment.store_id.as_uuid(),
            ADJUST_STOCK_OPERATION,
            request,
            inventory_snapshot(&item),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(item)
    }

    async fn list_stock(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<InventoryItemId>,
        limit: u16,
    ) -> Result<Option<Vec<InventoryItemView>>, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        if !store_exists(&mut transaction, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, InventoryItemRow>(
            "SELECT id, inventory_location_id, product_variant_id, on_hand_quantity, \
                    reserved_quantity, updated_at FROM commerce.inventory_items \
             WHERE store_id = $1 \
               AND ($2::uuid IS NULL OR id > $2) ORDER BY id ASC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(InventoryItemId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(rows.into_iter().map(inventory_item).collect()))
    }

    async fn create_reservation(
        &self,
        actor: &MachineActor,
        reservation: &InventoryReservation,
        request: &IdempotencyRequest,
    ) -> Result<InventoryReservationId, ApplicationError> {
        let channel_id = actor
            .sales_channel_id
            .ok_or_else(invalid_inventory_selection)?;
        let mut transaction = self.begin_for_machine(actor).await?;
        if let Some(snapshot) = reserve_idempotency(
            &mut transaction,
            actor.store_id.as_uuid(),
            CREATE_RESERVATION_OPERATION,
            request,
        )
        .await?
        {
            return replay_id(&snapshot).map(InventoryReservationId::from_uuid);
        }
        require_active_machine_context(&mut transaction, actor, channel_id.as_uuid()).await?;
        sqlx::query(
            "INSERT INTO commerce.inventory_reservations \
             (id, store_id, sales_channel_id, expires_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(reservation.id().as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(reservation.expires_at())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;

        let mut lines = reservation.lines().iter().collect::<Vec<_>>();
        lines.sort_by_key(|line| line.inventory_item_id());
        for line in lines {
            let locked = lock_inventory_item_by_id(
                &mut transaction,
                actor.store_id,
                line.inventory_item_id(),
            )
            .await?
            .ok_or_else(invalid_inventory_selection)?;
            let balance = StockBalance::new(locked.2, locked.3)?.reserve(line.quantity())?;
            update_inventory_balance(&mut transaction, locked.0, balance).await?;
            sqlx::query(
                "INSERT INTO commerce.inventory_reservation_lines \
                 (store_id, reservation_id, inventory_item_id, quantity) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(actor.store_id.as_uuid())
            .bind(reservation.id().as_uuid())
            .bind(locked.0)
            .bind(line.quantity())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            insert_inventory_transaction(
                &mut transaction,
                actor.store_id,
                locked.0,
                Some("reservation"),
                Some(reservation.id().as_uuid()),
                0,
                line.quantity(),
                balance,
                None,
                None,
            )
            .await?;
        }
        complete_id(
            &mut transaction,
            actor.store_id.as_uuid(),
            CREATE_RESERVATION_OPERATION,
            request,
            reservation.id().as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(reservation.id())
    }

    async fn transition_reservation(
        &self,
        actor: &MachineActor,
        reservation_id: InventoryReservationId,
        transition: InventoryReservationTransition,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<InventoryReservationDetail, ApplicationError> {
        let operation = match transition {
            InventoryReservationTransition::Release => RELEASE_RESERVATION_OPERATION,
            InventoryReservationTransition::Consume => CONSUME_RESERVATION_OPERATION,
        };
        let mut transaction = self.begin_for_machine(actor).await?;
        if let Some(snapshot) = reserve_idempotency(
            &mut transaction,
            actor.store_id.as_uuid(),
            operation,
            request,
        )
        .await?
        {
            return replay_reservation(&snapshot);
        }
        let row = lock_reservation(&mut transaction, actor, reservation_id).await?;
        if row.0 != "active" {
            return Err(reservation_not_active());
        }
        let effective_transition = if now >= row.1 {
            ReservationClosure::Expired
        } else {
            match transition {
                InventoryReservationTransition::Release => ReservationClosure::Released,
                InventoryReservationTransition::Consume => ReservationClosure::Consumed,
            }
        };
        close_reservation(
            &mut transaction,
            actor.store_id,
            reservation_id,
            effective_transition,
            now,
        )
        .await?;
        let detail = InventoryReservationDetail {
            id: reservation_id,
            status: effective_transition.status(),
            expires_at: row.1,
            closed_at: Some(now),
        };
        complete_snapshot(
            &mut transaction,
            actor.store_id.as_uuid(),
            operation,
            request,
            reservation_snapshot(&detail),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn expire_due_reservations(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<u16, ApplicationError> {
        let mut transaction = self.begin_for_store_actor(actor).await?;
        require_store(&mut transaction, store_id).await?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM commerce.inventory_reservations \
             WHERE store_id = $1 \
               AND status = 'active' AND expires_at <= $2 \
             ORDER BY expires_at ASC, id ASC FOR UPDATE SKIP LOCKED LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        for id in &ids {
            close_reservation(
                &mut transaction,
                store_id,
                InventoryReservationId::from_uuid(*id),
                ReservationClosure::Expired,
                now,
            )
            .await?;
        }
        transaction.commit().await.map_err(database_error)?;
        u16::try_from(ids.len()).map_err(|error| ApplicationError::Unexpected(error.into()))
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReservationClosure {
    Released,
    Consumed,
    Expired,
}

impl ReservationClosure {
    const fn status(self) -> InventoryReservationStatus {
        match self {
            Self::Released => InventoryReservationStatus::Released,
            Self::Consumed => InventoryReservationStatus::Consumed,
            Self::Expired => InventoryReservationStatus::Expired,
        }
    }
}

async fn set_context(
    transaction: &mut Transaction<'static, Postgres>,
    user_id: Option<Uuid>,
    store_id: Uuid,
) -> Result<(), ApplicationError> {
    if let Some(user_id) = user_id {
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(user_id.to_string())
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
    }
    sqlx::query("SELECT set_config('app.store_id', $1, true)")
        .bind(store_id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(())
}

async fn require_store(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    if store_exists(transaction, store_id).await? {
        Ok(())
    } else {
        Err(ApplicationError::NotFound {
            resource: "store",
            id: store_id.as_uuid().to_string(),
        })
    }
}

async fn store_exists(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM commerce.stores WHERE id = $1)")
        .bind(store_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)
}

async fn require_active_machine_context(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    sales_channel_id: Uuid,
) -> Result<(), ApplicationError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.stores AS store \
         INNER JOIN commerce.sales_channels AS channel \
           ON channel.store_id = store.id \
         WHERE store.id = $1 \
           AND store.status = 'active' AND channel.id = $2 AND channel.status = 'active')",
    )
    .bind(actor.store_id.as_uuid())
    .bind(sales_channel_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if valid {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

async fn lock_inventory_item_by_location_variant(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    location_id: InventoryLocationId,
    variant_id: ProductVariantId,
) -> Result<Option<LockedInventoryItemRow>, ApplicationError> {
    sqlx::query_as(
        "SELECT id, on_hand_quantity, reserved_quantity \
         FROM commerce.inventory_items WHERE store_id = $1 \
           AND inventory_location_id = $2 AND product_variant_id = $3 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(location_id.as_uuid())
    .bind(variant_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)
}

async fn lock_inventory_item_by_id(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    inventory_item_id: InventoryItemId,
) -> Result<Option<(Uuid, Uuid, i64, i64)>, ApplicationError> {
    sqlx::query_as(
        "SELECT item.id, item.product_variant_id, item.on_hand_quantity, \
                item.reserved_quantity FROM commerce.inventory_items AS item \
         INNER JOIN commerce.inventory_locations AS location \
           ON location.store_id = item.store_id \
          AND location.id = item.inventory_location_id \
         WHERE item.store_id = $1 AND item.id = $2 \
           AND location.archived_at IS NULL FOR UPDATE OF item",
    )
    .bind(store_id.as_uuid())
    .bind(inventory_item_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)
}

async fn update_inventory_balance(
    transaction: &mut Transaction<'static, Postgres>,
    inventory_item_id: Uuid,
    balance: StockBalance,
) -> Result<OffsetDateTime, ApplicationError> {
    sqlx::query_scalar(
        "UPDATE commerce.inventory_items SET on_hand_quantity = $2, reserved_quantity = $3, \
                updated_at = CURRENT_TIMESTAMP WHERE id = $1 RETURNING updated_at",
    )
    .bind(inventory_item_id)
    .bind(balance.on_hand())
    .bind(balance.reserved())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)
}

#[allow(clippy::too_many_arguments)]
async fn insert_inventory_transaction(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    inventory_item_id: Uuid,
    reference_type: Option<&str>,
    reference_id: Option<Uuid>,
    on_hand_delta: i64,
    reserved_delta: i64,
    balance: StockBalance,
    note: Option<&str>,
    actor_user_id: Option<Uuid>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.inventory_transactions \
         (id, store_id, inventory_item_id, reference_type, reference_id, \
          on_hand_delta_quantity, reserved_delta_quantity, resulting_on_hand_quantity, \
          resulting_reserved_quantity, note, actor_user_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(inventory_item_id)
    .bind(reference_type)
    .bind(reference_id)
    .bind(on_hand_delta)
    .bind(reserved_delta)
    .bind(balance.on_hand())
    .bind(balance.reserved())
    .bind(note)
    .bind(actor_user_id)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn lock_reservation(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    reservation_id: InventoryReservationId,
) -> Result<ReservationRow, ApplicationError> {
    sqlx::query_as(
        "SELECT status::text, expires_at, closed_at, sales_channel_id \
         FROM commerce.inventory_reservations \
         WHERE store_id = $1 AND id = $2 \
           AND sales_channel_id = $3 FOR UPDATE",
    )
    .bind(actor.store_id.as_uuid())
    .bind(reservation_id.as_uuid())
    .bind(actor.sales_channel_id.map(|id| id.as_uuid()))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ApplicationError::NotFound {
        resource: "inventory_reservation",
        id: reservation_id.as_uuid().to_string(),
    })
}

pub(super) async fn close_reservation(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    reservation_id: InventoryReservationId,
    closure: ReservationClosure,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let lines = sqlx::query_as::<_, (Uuid, i64, i64, i64)>(
        "SELECT item.id, line.quantity, item.on_hand_quantity, item.reserved_quantity \
         FROM commerce.inventory_reservation_lines AS line \
         INNER JOIN commerce.inventory_items AS item \
           ON item.store_id = line.store_id AND item.id = line.inventory_item_id \
         WHERE line.store_id = $1 \
           AND line.reservation_id = $2 ORDER BY item.id ASC FOR UPDATE OF item",
    )
    .bind(store_id.as_uuid())
    .bind(reservation_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    for (inventory_item_id, quantity, on_hand, reserved) in lines {
        let current = StockBalance::new(on_hand, reserved)?;
        let balance = match closure {
            ReservationClosure::Consumed => current.consume(quantity)?,
            ReservationClosure::Released | ReservationClosure::Expired => {
                current.release(quantity)?
            }
        };
        update_inventory_balance(transaction, inventory_item_id, balance).await?;
        let on_hand_delta = if matches!(closure, ReservationClosure::Consumed) {
            -quantity
        } else {
            0
        };
        insert_inventory_transaction(
            transaction,
            store_id,
            inventory_item_id,
            Some("reservation"),
            Some(reservation_id.as_uuid()),
            on_hand_delta,
            -quantity,
            balance,
            None,
            None,
        )
        .await?;
    }
    let result = sqlx::query(
        "UPDATE commerce.inventory_reservations \
         SET status = $3::commerce.inventory_reservation_status, closed_at = $4, updated_at = $4 \
         WHERE store_id = $1 AND id = $2 AND status = 'active'",
    )
    .bind(store_id.as_uuid())
    .bind(reservation_id.as_uuid())
    .bind(closure.status().as_str())
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(reservation_not_active())
    }
}

async fn reserve_idempotency(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: Uuid,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Value>, ApplicationError> {
    idempotency::reserve(
        transaction,
        &IdempotencyScope::Store(store_id),
        operation,
        request,
    )
    .await
}

async fn complete_id(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: Uuid,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: Uuid,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::Store(store_id),
        operation,
        request,
        201,
        json!({ "id": id }),
    )
    .await
}

async fn complete_snapshot(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: Uuid,
    operation: &'static str,
    request: &IdempotencyRequest,
    snapshot: Value,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::Store(store_id),
        operation,
        request,
        200,
        snapshot,
    )
    .await
}

fn location_item(row: LocationRow) -> Result<InventoryLocationItem, ApplicationError> {
    Ok(InventoryLocationItem {
        id: InventoryLocationId::from_uuid(row.0),
        code: row.1,
        name: row.2,
        archived_at: row.3,
        created_at: row.4,
        updated_at: row.5,
    })
}

fn inventory_item(row: InventoryItemRow) -> InventoryItemView {
    InventoryItemView {
        id: InventoryItemId::from_uuid(row.0),
        inventory_location_id: InventoryLocationId::from_uuid(row.1),
        product_variant_id: ProductVariantId::from_uuid(row.2),
        on_hand_quantity: row.3,
        reserved_quantity: row.4,
        available_quantity: row.3 - row.4,
        updated_at: row.5,
    }
}

fn inventory_snapshot(item: &InventoryItemView) -> Value {
    json!({
        "id": item.id.as_uuid(),
        "inventory_location_id": item.inventory_location_id.as_uuid(),
        "product_variant_id": item.product_variant_id.as_uuid(),
        "on_hand_quantity": item.on_hand_quantity,
        "reserved_quantity": item.reserved_quantity,
        "available_quantity": item.available_quantity,
        "updated_at": format_time(item.updated_at),
    })
}

fn replay_inventory_item(snapshot: &Value) -> Result<InventoryItemView, ApplicationError> {
    Ok(InventoryItemView {
        id: InventoryItemId::from_uuid(snapshot_uuid(snapshot, "id")?),
        inventory_location_id: InventoryLocationId::from_uuid(snapshot_uuid(
            snapshot,
            "inventory_location_id",
        )?),
        product_variant_id: ProductVariantId::from_uuid(snapshot_uuid(
            snapshot,
            "product_variant_id",
        )?),
        on_hand_quantity: snapshot_i64(snapshot, "on_hand_quantity")?,
        reserved_quantity: snapshot_i64(snapshot, "reserved_quantity")?,
        available_quantity: snapshot_i64(snapshot, "available_quantity")?,
        updated_at: snapshot_time(snapshot, "updated_at")?,
    })
}

fn reservation_snapshot(detail: &InventoryReservationDetail) -> Value {
    json!({
        "id": detail.id.as_uuid(),
        "status": detail.status.as_str(),
        "expires_at": format_time(detail.expires_at),
        "closed_at": detail.closed_at.map(format_time),
    })
}

fn replay_reservation(snapshot: &Value) -> Result<InventoryReservationDetail, ApplicationError> {
    let status = snapshot
        .get("status")
        .and_then(Value::as_str)
        .and_then(InventoryReservationStatus::parse)
        .ok_or_else(invalid_snapshot)?;
    Ok(InventoryReservationDetail {
        id: InventoryReservationId::from_uuid(snapshot_uuid(snapshot, "id")?),
        status,
        expires_at: snapshot_time(snapshot, "expires_at")?,
        closed_at: snapshot
            .get("closed_at")
            .filter(|value| !value.is_null())
            .map(|_| snapshot_time(snapshot, "closed_at"))
            .transpose()?,
    })
}

fn replay_id(snapshot: &Value) -> Result<Uuid, ApplicationError> {
    snapshot_uuid(snapshot, "id")
}

fn snapshot_uuid(snapshot: &Value, field: &str) -> Result<Uuid, ApplicationError> {
    snapshot
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(invalid_snapshot)
}

fn snapshot_i64(snapshot: &Value, field: &str) -> Result<i64, ApplicationError> {
    snapshot
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(invalid_snapshot)
}

fn snapshot_time(snapshot: &Value, field: &str) -> Result<OffsetDateTime, ApplicationError> {
    snapshot
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| {
            OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
        })
        .ok_or_else(invalid_snapshot)
}

fn format_time(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .expect("OffsetDateTime must format as RFC 3339")
}

fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("invalid inventory idempotency snapshot"))
}

fn invalid_inventory_selection() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "inventory",
            reason: "location, variant, and inventory item must belong to the Store".into(),
        }],
    }
}

fn reservation_not_active() -> ApplicationError {
    ApplicationError::Conflict {
        code: "inventory_reservation_not_active",
        message: "the inventory reservation is no longer active",
    }
}

fn map_location_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error
        && database.constraint() == Some("inventory_locations_store_id_code_key")
    {
        return ApplicationError::Conflict {
            code: "inventory_location_code_taken",
            message: "the inventory location code is already in use for this Store",
        };
    }
    database_error(error)
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            ApplicationError::Unavailable {
                service: "postgresql",
                source: error.into(),
            }
        }
        _ => ApplicationError::Unexpected(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chaos_application::{
        inventory::{
            AdjustStockInput, CreateInventoryLocationInput, InventoryManagement,
            ReserveInventoryInput, ReserveInventoryLineInput, TransitionInventoryReservationInput,
        },
        ports::IdempotencyRequest,
        store::StoreQueries,
    };
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        identity::UserId,
        store::{PublishableKeyId, SalesChannelId},
    };
    use sqlx::postgres::PgPoolOptions;
    use time::Duration;

    use super::*;

    fn request(key: impl Into<String>, fingerprint: u8) -> IdempotencyRequest {
        IdempotencyRequest {
            key: key.into(),
            request_fingerprint: [fingerprint; 32],
        }
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn inventory_is_concurrency_safe_isolated_and_append_only() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(&database_url)
            .await
            .unwrap();
        let runtime_pool = PgPoolOptions::new()
            .max_connections(6)
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("SET ROLE chaos_runtime")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .unwrap();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();
        let user_id = UserId::new();
        let other_user_id = UserId::new();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let product_id = ProductId::new();
        let variant_id = ProductVariantId::new();

        for (id, email) in [
            (user_id, format!("inventory-{suffix}@example.com")),
            (
                other_user_id,
                format!("inventory-other-{suffix}@example.com"),
            ),
        ] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(email)
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        for (id, code) in [
            (store_id, "inventory-store"),
            (other_store_id, "other-inventory"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.stores \
                 (id, code, name, status) \
                 VALUES ($1, $2, 'Inventory Store', 'active')",
            )
            .bind(id.as_uuid())
            .bind(format!("{code}-{suffix}"))
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (store, user) in [(store_id, user_id), (other_store_id, other_user_id)] {
            sqlx::query(
                "INSERT INTO commerce.store_memberships \
                 (store_id, user_id, role) VALUES ($1, $2, 'owner')",
            )
            .bind(store.as_uuid())
            .bind(user.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO commerce.sales_channels \
             (id, store_id, code, name, is_default) \
             VALUES ($1, $2, 'web', 'Web', true)",
        )
        .bind(channel_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.products \
             (id, store_id, handle, title, status) \
             VALUES ($1, $2, 'inventory-product', 'Inventory Product', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.product_variants \
             (id, store_id, product_id, title, track_inventory) \
             VALUES ($1, $2, $3, 'Default', true)",
        )
        .bind(variant_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let queries = StoreQueries::new(Arc::new(
            crate::repositories::PostgresStoreReadRepository::new(runtime_pool.clone()),
        ));
        let actor = queries.authorize(user_id, store_id).await.unwrap();
        let other_actor = queries
            .authorize(other_user_id, other_store_id)
            .await
            .unwrap();
        let service = Arc::new(InventoryManagement::new(Arc::new(
            PostgresInventoryRepository::new(runtime_pool.clone()),
        )));
        let location_request = request(format!("location-{suffix}"), 1);
        let location_id = service
            .create_location(CreateInventoryLocationInput {
                actor: AdminActor::Store(actor),
                store_id,
                code: "primary".into(),
                name: "Primary Warehouse".into(),
                idempotency: location_request,
            })
            .await
            .unwrap();
        let adjustment_key = format!("adjust-{suffix}");
        let stock = service
            .adjust_stock(AdjustStockInput {
                actor: AdminActor::Store(actor),
                store_id,
                inventory_location_id: location_id,
                product_variant_id: variant_id,
                delta_quantity: 10,
                note: "Initial receipt".into(),
                idempotency: request(adjustment_key.clone(), 2),
            })
            .await
            .unwrap();
        let replay = service
            .adjust_stock(AdjustStockInput {
                actor: AdminActor::Store(actor),
                store_id,
                inventory_location_id: location_id,
                product_variant_id: variant_id,
                delta_quantity: 10,
                note: "Initial receipt".into(),
                idempotency: request(adjustment_key, 2),
            })
            .await
            .unwrap();
        assert_eq!(replay.on_hand_quantity, 10);
        assert!(
            service
                .adjust_stock(AdjustStockInput {
                    actor: AdminActor::Store(actor),
                    store_id,
                    inventory_location_id: location_id,
                    product_variant_id: variant_id,
                    delta_quantity: -11,
                    note: "Invalid shrinkage".into(),
                    idempotency: request(format!("invalid-adjust-{suffix}"), 3),
                })
                .await
                .is_err()
        );

        let machine = MachineActor {
            publishable_key_id: PublishableKeyId::new(),
            store_id,
            sales_channel_id: Some(channel_id),
            created_by_user_id: user_id,
        };
        let now = OffsetDateTime::now_utc();
        let first_input = ReserveInventoryInput {
            actor: machine.clone(),
            now,
            expires_at: now + Duration::minutes(15),
            lines: vec![ReserveInventoryLineInput {
                inventory_item_id: stock.id,
                quantity: 7,
            }],
            idempotency: request(format!("reserve-a-{suffix}"), 4),
        };
        let second_input = ReserveInventoryInput {
            actor: machine.clone(),
            now,
            expires_at: now + Duration::minutes(15),
            lines: vec![ReserveInventoryLineInput {
                inventory_item_id: stock.id,
                quantity: 7,
            }],
            idempotency: request(format!("reserve-b-{suffix}"), 5),
        };
        let first_service = service.clone();
        let first = tokio::spawn(async move { first_service.reserve(first_input).await });
        let second_service = service.clone();
        let second = tokio::spawn(async move { second_service.reserve(second_input).await });
        let outcomes = [first.await.unwrap(), second.await.unwrap()];
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        let reservation_id = outcomes.into_iter().find_map(Result::ok).unwrap();
        let after_reserve = service
            .list_stock(AdminActor::Store(actor), store_id, None, 100)
            .await
            .unwrap();
        assert_eq!(after_reserve.items[0].reserved_quantity, 7);
        assert_eq!(after_reserve.items[0].available_quantity, 3);

        service
            .consume(TransitionInventoryReservationInput {
                actor: machine.clone(),
                reservation_id,
                now: now + Duration::minutes(1),
                idempotency: request(format!("consume-{suffix}"), 6),
            })
            .await
            .unwrap();
        let after_consume = service
            .list_stock(AdminActor::Store(actor), store_id, None, 100)
            .await
            .unwrap();
        assert_eq!(after_consume.items[0].on_hand_quantity, 3);
        assert_eq!(after_consume.items[0].reserved_quantity, 0);

        let expiring_id = service
            .reserve(ReserveInventoryInput {
                actor: machine,
                now,
                expires_at: now + Duration::minutes(5),
                lines: vec![ReserveInventoryLineInput {
                    inventory_item_id: stock.id,
                    quantity: 1,
                }],
                idempotency: request(format!("reserve-expiring-{suffix}"), 7),
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .expire_due(actor, store_id, now + Duration::minutes(6), 100)
                .await
                .unwrap(),
            1
        );
        let expired_status: String = sqlx::query_scalar(
            "SELECT status::text FROM commerce.inventory_reservations WHERE id = $1",
        )
        .bind(expiring_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(expired_status, "expired");

        assert!(
            service
                .list_stock(AdminActor::Store(other_actor), store_id, None, 100)
                .await
                .is_err()
        );
        assert!(
            service
                .list_stock(AdminActor::Store(actor), other_store_id, None, 100)
                .await
                .is_err()
        );
        let ledger_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commerce.inventory_transactions \
             WHERE store_id = $1",
        )
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(ledger_count, 5);

        let mut runtime_connection = runtime_pool.acquire().await.unwrap();
        sqlx::query("SELECT set_config('app.store_id', $1, false)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *runtime_connection)
            .await
            .unwrap();
        assert!(
            sqlx::query(
                "UPDATE commerce.inventory_transactions SET note = 'tampered' \
                 WHERE store_id = $1",
            )
            .bind(store_id.as_uuid())
            .execute(&mut *runtime_connection)
            .await
            .is_err()
        );
    }
}
