// Inventory location, balance, and reservation idempotency snapshots.

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
