// Inventory reservation for a pending Order/payment attempt.

async fn reserve_inventory(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    channel_id: SalesChannelId,
    cart: &Cart,
    expires_at: OffsetDateTime,
) -> Result<Option<InventoryReservationId>, ApplicationError> {
    if !cart.lines().iter().any(CartLine::track_inventory) {
        return Ok(None);
    }
    let reservation_id = InventoryReservationId::new();
    sqlx::query(
        "INSERT INTO commerce.inventory_reservations \
         (id, store_id, sales_channel_id, expires_at) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(reservation_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(expires_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    for line in cart.lines().iter().filter(|line| line.track_inventory()) {
        let stocks = sqlx::query_as::<_, (Uuid, i64, i64)>(
            "SELECT stock.id, stock.on_hand_quantity, stock.reserved_quantity \
             FROM commerce.inventory_items AS stock \
             INNER JOIN commerce.inventory_locations AS location \
               ON location.store_id = stock.store_id AND location.id = stock.inventory_location_id \
             WHERE stock.store_id = $1 \
               AND stock.product_variant_id = $2 AND location.archived_at IS NULL \
               AND stock.on_hand_quantity > stock.reserved_quantity \
             ORDER BY stock.id ASC FOR UPDATE OF stock",
        )
        .bind(actor.store_id.as_uuid())
        .bind(line.product_variant_id().as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let mut remaining = i64::from(line.quantity());
        for (inventory_item_id, on_hand, reserved) in stocks {
            if remaining == 0 {
                break;
            }
            let current = InventoryBalance::new(on_hand, reserved)?;
            let allocated = remaining.min(current.available());
            if allocated == 0 {
                continue;
            }
            let balance = current.reserve(allocated)?;
            sqlx::query(
                "UPDATE commerce.inventory_items SET reserved_quantity = $1, \
                        updated_at = CURRENT_TIMESTAMP WHERE id = $2",
            )
            .bind(balance.reserved())
            .bind(inventory_item_id)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO commerce.inventory_reservation_lines \
                 (store_id, reservation_id, inventory_item_id, quantity) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(actor.store_id.as_uuid())
            .bind(reservation_id.as_uuid())
            .bind(inventory_item_id)
            .bind(allocated)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO commerce.inventory_transactions \
                 (id, store_id, inventory_item_id, reference_type, reference_id, \
                  on_hand_delta_quantity, reserved_delta_quantity, resulting_on_hand_quantity, \
                  resulting_reserved_quantity) \
                 VALUES ($1, $2, $3, 'reservation', $4, 0, $5, $6, $7)",
            )
            .bind(Uuid::now_v7())
            .bind(actor.store_id.as_uuid())
            .bind(inventory_item_id)
            .bind(reservation_id.as_uuid())
            .bind(allocated)
            .bind(balance.on_hand())
            .bind(balance.reserved())
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            remaining -= allocated;
        }
        if remaining != 0 {
            return Err(insufficient_inventory(line.product_variant_id()));
        }
    }
    Ok(Some(reservation_id))
}
