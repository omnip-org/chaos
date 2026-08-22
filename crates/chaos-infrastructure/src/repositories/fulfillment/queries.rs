// Fulfillment allocation, shipment, and return detail queries.

async fn load_allocations(
    tx: &mut Transaction<'static, Postgres>,
    _account_id: Uuid,
    store_id: StoreId,
    fulfillment_id: FulfillmentId,
) -> Result<Vec<FulfillmentAllocation>, ApplicationError> {
    sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT product_variant_id, quantity FROM commerce.fulfillment_lines \
         WHERE store_id = $1 AND fulfillment_id = $2 \
         ORDER BY product_variant_id",
    )
    .bind(store_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|(variant_id, quantity)| {
        Ok(FulfillmentAllocation {
            product_variant_id: ProductVariantId::from_uuid(variant_id),
            quantity: u32::try_from(quantity).map_err(unexpected_conversion)?,
        })
    })
    .collect()
}

async fn load_fulfillment(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    fulfillment_id: FulfillmentId,
) -> Result<Option<FulfillmentDetail>, ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Option<String>,
            Option<String>,
            OffsetDateTime,
            OffsetDateTime,
        ),
    >(
        "SELECT order_id, status::text, carrier, tracking_number, created_at, updated_at \
         FROM commerce.fulfillments WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(fulfillment_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    let Some((order_id, status, carrier, tracking_number, created_at, updated_at)) = row else {
        return Ok(None);
    };
    let allocations = load_allocations(tx, account_id, store_id, fulfillment_id)
        .await?
        .into_iter()
        .map(|line| FulfillmentAllocationInput {
            product_variant_id: line.product_variant_id,
            quantity: line.quantity,
        })
        .collect();
    Ok(Some(FulfillmentDetail {
        id: fulfillment_id,
        order_id: OrderId::from_uuid(order_id),
        status: FulfillmentStatus::parse(&status).ok_or_else(corrupt_state)?,
        carrier,
        tracking_number,
        allocations,
        created_at,
        updated_at,
    }))
}

async fn load_return(
    tx: &mut Transaction<'static, Postgres>,
    _account_id: Uuid,
    store_id: StoreId,
    return_id: ReturnId,
) -> Result<Option<ReturnDetail>, ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Option<Uuid>,
            i64,
            String,
            OffsetDateTime,
            OffsetDateTime,
        ),
    >(
        "SELECT order_id, status::text, refund_id, refund_amount_minor, currency::text, \
                created_at, updated_at FROM commerce.returns \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    let Some((order_id, status, refund_id, refund_amount_minor, currency, created_at, updated_at)) =
        row
    else {
        return Ok(None);
    };
    let lines = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT product_variant_id, quantity FROM commerce.return_lines \
         WHERE store_id = $1 AND return_id = $2 \
         ORDER BY product_variant_id",
    )
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|(variant_id, quantity)| {
        Ok(ReturnLineInput {
            product_variant_id: ProductVariantId::from_uuid(variant_id),
            quantity: u32::try_from(quantity).map_err(unexpected_conversion)?,
        })
    })
    .collect::<Result<Vec<_>, ApplicationError>>()?;
    Ok(Some(ReturnDetail {
        id: return_id,
        order_id: OrderId::from_uuid(order_id),
        status: ReturnStatus::parse(&status).ok_or_else(corrupt_state)?,
        lines,
        refund_id: refund_id.map(RefundId::from_uuid),
        refund_amount_minor,
        currency: CurrencyCode::parse(&currency)?,
        created_at,
        updated_at,
    }))
}
