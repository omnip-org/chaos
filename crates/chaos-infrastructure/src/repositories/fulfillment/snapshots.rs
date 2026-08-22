// Fulfillment and return idempotency snapshots and reconstruction.

async fn insert_return_outbox(
    tx: &mut Transaction<'static, Postgres>,
    _account_id: Uuid,
    store_id: StoreId,
    return_id: ReturnId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO integration.event_outbox \
         (id, store_id, aggregate_type, aggregate_id, event_type, payload) \
         VALUES ($1, $2, 'return', $3, 'return.completed', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .bind(serde_json::json!({
        "return_id": return_id.as_uuid(),
        "order_id": order_id.as_uuid(),
    }))
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct FulfillmentSnapshot {
    id: Uuid,
    order_id: Uuid,
    status: String,
    carrier: Option<String>,
    tracking_number: Option<String>,
    allocations: Vec<LineSnapshot>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct ReturnSnapshot {
    id: Uuid,
    order_id: Uuid,
    status: String,
    lines: Vec<LineSnapshot>,
    refund_id: Option<Uuid>,
    refund_amount_minor: i64,
    currency: String,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct LineSnapshot {
    product_variant_id: Uuid,
    quantity: u32,
}

fn fulfillment_snapshot(detail: &FulfillmentDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(FulfillmentSnapshot {
        id: detail.id.as_uuid(),
        order_id: detail.order_id.as_uuid(),
        status: detail.status.as_str().into(),
        carrier: detail.carrier.clone(),
        tracking_number: detail.tracking_number.clone(),
        allocations: detail
            .allocations
            .iter()
            .map(|line| LineSnapshot {
                product_variant_id: line.product_variant_id.as_uuid(),
                quantity: line.quantity,
            })
            .collect(),
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn return_snapshot(detail: &ReturnDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(ReturnSnapshot {
        id: detail.id.as_uuid(),
        order_id: detail.order_id.as_uuid(),
        status: detail.status.as_str().into(),
        lines: detail
            .lines
            .iter()
            .map(|line| LineSnapshot {
                product_variant_id: line.product_variant_id.as_uuid(),
                quantity: line.quantity,
            })
            .collect(),
        refund_id: detail.refund_id.map(RefundId::as_uuid),
        refund_amount_minor: detail.refund_amount_minor,
        currency: detail.currency.as_str().into(),
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_fulfillment(value: Value) -> Result<FulfillmentDetail, ApplicationError> {
    let snapshot: FulfillmentSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(FulfillmentDetail {
        id: FulfillmentId::from_uuid(snapshot.id),
        order_id: OrderId::from_uuid(snapshot.order_id),
        status: FulfillmentStatus::parse(&snapshot.status).ok_or_else(corrupt_state)?,
        carrier: snapshot.carrier,
        tracking_number: snapshot.tracking_number,
        allocations: snapshot
            .allocations
            .into_iter()
            .map(|line| FulfillmentAllocationInput {
                product_variant_id: ProductVariantId::from_uuid(line.product_variant_id),
                quantity: line.quantity,
            })
            .collect(),
        created_at: parse_time(&snapshot.created_at)?,
        updated_at: parse_time(&snapshot.updated_at)?,
    })
}

fn replay_return(value: Value) -> Result<ReturnDetail, ApplicationError> {
    let snapshot: ReturnSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(ReturnDetail {
        id: ReturnId::from_uuid(snapshot.id),
        order_id: OrderId::from_uuid(snapshot.order_id),
        status: ReturnStatus::parse(&snapshot.status).ok_or_else(corrupt_state)?,
        lines: snapshot
            .lines
            .into_iter()
            .map(|line| ReturnLineInput {
                product_variant_id: ProductVariantId::from_uuid(line.product_variant_id),
                quantity: line.quantity,
            })
            .collect(),
        refund_id: snapshot.refund_id.map(RefundId::from_uuid),
        refund_amount_minor: snapshot.refund_amount_minor,
        currency: CurrencyCode::parse(&snapshot.currency)?,
        created_at: parse_time(&snapshot.created_at)?,
        updated_at: parse_time(&snapshot.updated_at)?,
    })
}
