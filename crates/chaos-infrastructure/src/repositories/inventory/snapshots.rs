use serde_json::json;

fn variant_inventory(row: VariantInventoryRow) -> VariantInventoryView {
    VariantInventoryView {
        product_variant_id: ProductVariantId::from_uuid(row.0),
        on_hand_quantity: row.1,
        updated_at: row.2,
    }
}

fn inventory_snapshot(item: &VariantInventoryView) -> Value {
    json!({
        "product_variant_id": item.product_variant_id.as_uuid(),
        "on_hand_quantity": item.on_hand_quantity,
        "updated_at": format_time(item.updated_at),
    })
}

fn replay_variant_inventory(snapshot: &Value) -> Result<VariantInventoryView, ApplicationError> {
    Ok(VariantInventoryView {
        product_variant_id: ProductVariantId::from_uuid(snapshot_uuid(
            snapshot,
            "product_variant_id",
        )?),
        on_hand_quantity: snapshot_i64(snapshot, "on_hand_quantity")?,
        updated_at: snapshot_time(snapshot, "updated_at")?,
    })
}

fn format_time(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .expect("OffsetDateTime must format as RFC 3339")
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

fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("invalid inventory idempotency snapshot"))
}
