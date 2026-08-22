// Inventory reservation creation, release, consumption, expiry, and balance mutation helpers.

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
    balance: InventoryBalance,
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
    balance: InventoryBalance,
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
        let current = InventoryBalance::new(on_hand, reserved)?;
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
