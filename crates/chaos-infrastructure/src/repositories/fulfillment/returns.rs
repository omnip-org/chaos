// Return validation, return receipt, restocking, and refund allocation.

fn payload_uuid(payload: &Value, field: &'static str) -> Result<Uuid, ApplicationError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| invalid_event_payload("required UUID field is missing or invalid"))
}

fn invalid_event_payload(message: &'static str) -> ApplicationError {
    ApplicationError::Conflict {
        code: "invalid_fulfillment_event",
        message,
    }
}

fn unsupported_event(event_type: &str) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "unsupported fulfillment event type: {event_type}"
    ))
}

async fn lock_confirmed_order(
    tx: &mut Transaction<'static, Postgres>,
    _account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status::text FROM commerce.orders WHERE store_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    match status.as_deref() {
        Some("confirmed") => Ok(()),
        Some(_) => Err(ApplicationError::Conflict {
            code: "order_not_confirmed",
            message: "the Order is not confirmed",
        }),
        None => Err(ApplicationError::NotFound {
            resource: "order",
            id: order_id.as_uuid().to_string(),
        }),
    }
}

async fn validate_fulfillment_quantities(
    tx: &mut Transaction<'static, Postgres>,
    _account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
    allocations: &[FulfillmentAllocation],
) -> Result<(), ApplicationError> {
    for line in allocations {
        let quantities = sqlx::query_as::<_, (i64, i64)>(
            "SELECT order_line.quantity::bigint, COALESCE(sum(fulfillment_line.quantity) \
                    FILTER (WHERE fulfillment_record.status <> 'cancelled'), 0)::bigint \
             FROM commerce.order_lines AS order_line \
             LEFT JOIN commerce.fulfillments AS fulfillment_record \
               ON fulfillment_record.store_id = order_line.store_id \
              AND fulfillment_record.order_id = $2 \
             LEFT JOIN commerce.fulfillment_lines AS fulfillment_line \
               ON fulfillment_line.store_id = fulfillment_record.store_id \
              AND fulfillment_line.fulfillment_id = fulfillment_record.id \
              AND fulfillment_line.product_variant_id = order_line.product_variant_id \
             WHERE order_line.store_id = $1 \
               AND order_line.order_id = $2 AND order_line.product_variant_id = $3 \
               AND order_line.requires_shipping \
             GROUP BY order_line.quantity",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ApplicationError::NotFound {
            resource: "order_line",
            id: line.product_variant_id.as_uuid().to_string(),
        })?;
        if quantities.1 + i64::from(line.quantity) > quantities.0 {
            return Err(ApplicationError::Conflict {
                code: "fulfillment_quantity_exceeded",
                message: "Fulfillment quantity exceeds the unfulfilled Order quantity",
            });
        }
    }
    Ok(())
}

async fn validate_return_quantities(
    tx: &mut Transaction<'static, Postgres>,
    _account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
    lines: &[ReturnLineInput],
) -> Result<(), ApplicationError> {
    for line in lines {
        let delivered: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(fulfillment_line.quantity), 0)::bigint \
             FROM commerce.fulfillment_lines AS fulfillment_line \
             INNER JOIN commerce.fulfillments AS fulfillment_record \
               ON fulfillment_record.store_id = fulfillment_line.store_id \
              AND fulfillment_record.id = fulfillment_line.fulfillment_id \
             WHERE fulfillment_record.store_id = $1 \
               AND fulfillment_record.order_id = $2 AND fulfillment_record.status = 'delivered' \
               AND fulfillment_line.product_variant_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)?;
        let returned: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(return_line.quantity), 0)::bigint \
             FROM commerce.return_lines AS return_line \
             INNER JOIN commerce.returns AS return_record \
               ON return_record.store_id = return_line.store_id AND return_record.id = return_line.return_id \
             WHERE return_record.store_id = $1 \
               AND return_record.order_id = $2 AND return_record.status <> 'rejected' \
               AND return_line.product_variant_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)?;
        if returned + i64::from(line.quantity) > delivered {
            return Err(ApplicationError::Conflict {
                code: "return_quantity_exceeded",
                message: "Return quantity exceeds the delivered quantity",
            });
        }
    }
    Ok(())
}

struct ReturnRefundLine {
    product_variant_id: ProductVariantId,
    quantity: u32,
    refund_amount_minor: i64,
}

async fn allocate_return_refund(
    tx: &mut Transaction<'static, Postgres>,
    _account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
    lines: &[ReturnLineInput],
) -> Result<(CurrencyCode, Vec<ReturnRefundLine>, i64), ApplicationError> {
    let currency_text: String = sqlx::query_scalar(
        "SELECT currency::text FROM commerce.orders \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let currency = CurrencyCode::parse(&currency_text)?;
    let mut allocated = Vec::with_capacity(lines.len());
    let mut total = 0_i64;
    for line in lines {
        let order_line = sqlx::query_as::<_, (i32, i64)>(
            "SELECT quantity, total_amount_minor FROM commerce.order_lines \
             WHERE store_id = $1 AND order_id = $2 \
               AND product_variant_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ApplicationError::NotFound {
            resource: "order_line",
            id: line.product_variant_id.as_uuid().to_string(),
        })?;
        let reserved = sqlx::query_as::<_, (i64, i64)>(
            "SELECT COALESCE(sum(return_line.quantity), 0)::bigint, \
                    COALESCE(sum(return_line.refund_amount_minor), 0)::bigint \
             FROM commerce.return_lines AS return_line \
             INNER JOIN commerce.returns AS return_record \
               ON return_record.store_id = return_line.store_id \
              AND return_record.id = return_line.return_id \
             WHERE return_record.store_id = $1 \
               AND return_record.order_id = $2 AND return_record.status <> 'rejected' \
               AND return_line.product_variant_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(line.product_variant_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)?;
        let amount = calculate_return_refund_amount(
            order_line.1,
            u32::try_from(order_line.0).map_err(unexpected_conversion)?,
            line.quantity,
            u32::try_from(reserved.0).map_err(unexpected_conversion)?,
            reserved.1,
        )?;
        total = total
            .checked_add(amount)
            .ok_or_else(refund_amount_overflow)?;
        allocated.push(ReturnRefundLine {
            product_variant_id: line.product_variant_id,
            quantity: line.quantity,
            refund_amount_minor: amount,
        });
    }
    Ok((currency, allocated, total))
}

fn validate_return_lines(lines: &[ReturnLineInput]) -> Result<(), ApplicationError> {
    if lines.is_empty() {
        return Err(validation("lines", "must contain at least one line"));
    }
    for (index, line) in lines.iter().enumerate() {
        if !(1..=999).contains(&line.quantity) {
            return Err(validation("quantity", "must be between 1 and 999"));
        }
        if lines[..index]
            .iter()
            .any(|prior| prior.product_variant_id == line.product_variant_id)
        {
            return Err(validation("lines", "must not repeat a Variant"));
        }
    }
    Ok(())
}

async fn receive_return(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    return_id: ReturnId,
    receipt: &[ReturnReceiptInput],
    actor_user_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let expected: Vec<Uuid> = sqlx::query_scalar(
        "SELECT product_variant_id FROM commerce.return_lines \
         WHERE store_id = $1 AND return_id = $2 \
         ORDER BY product_variant_id",
    )
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?;
    let mut actual = receipt
        .iter()
        .map(|line| line.product_variant_id.as_uuid())
        .collect::<Vec<_>>();
    actual.sort_unstable();
    if actual != expected {
        return Err(validation(
            "receipt",
            "must provide one disposition for every Return line",
        ));
    }
    for line in receipt {
        if line.disposition == ReturnDisposition::Restock && line.inventory_location_id.is_none() {
            return Err(validation(
                "inventory_location_id",
                "is required when disposition is restock",
            ));
        }
        sqlx::query(
            "UPDATE commerce.return_lines SET disposition = $3::commerce.return_disposition, \
                    inventory_location_id = $4 WHERE store_id = $1 \
                    AND return_id = $2 AND product_variant_id = $5",
        )
        .bind(store_id.as_uuid())
        .bind(return_id.as_uuid())
        .bind(line.disposition.as_str())
        .bind(line.inventory_location_id.map(InventoryLocationId::as_uuid))
        .bind(line.product_variant_id.as_uuid())
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
        if line.disposition == ReturnDisposition::Restock {
            restock_return_line(
                tx,
                account_id,
                store_id,
                return_id,
                line.product_variant_id,
                line.inventory_location_id.unwrap(),
                actor_user_id,
                now,
            )
            .await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn restock_return_line(
    tx: &mut Transaction<'static, Postgres>,
    _account_id: Uuid,
    store_id: StoreId,
    return_id: ReturnId,
    variant_id: ProductVariantId,
    location_id: InventoryLocationId,
    actor_user_id: Uuid,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let quantity: i64 = sqlx::query_scalar(
        "SELECT quantity::bigint FROM commerce.return_lines WHERE store_id = $1 AND return_id = $2 AND product_variant_id = $3",
    )
    .bind(store_id.as_uuid())
    .bind(return_id.as_uuid())
    .bind(variant_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let stock = sqlx::query_as::<_, (Uuid, i64, i64)>(
        "SELECT id, on_hand_quantity, reserved_quantity FROM commerce.inventory_items \
         WHERE store_id = $1 AND inventory_location_id = $2 \
           AND product_variant_id = $3 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(location_id.as_uuid())
    .bind(variant_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    let (stock_id, current) = match stock {
        Some((id, on_hand, reserved)) => (id, InventoryBalance::new(on_hand, reserved)?),
        None => {
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM commerce.inventory_locations WHERE \
                 store_id = $1 AND id = $2 AND archived_at IS NULL)",
            )
            .bind(store_id.as_uuid())
            .bind(location_id.as_uuid())
            .fetch_one(&mut **tx)
            .await
            .map_err(database_error)?;
            if !active {
                return Err(ApplicationError::NotFound {
                    resource: "inventory_location",
                    id: location_id.as_uuid().to_string(),
                });
            }
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO commerce.inventory_items \
                 (id, store_id, inventory_location_id, product_variant_id) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(store_id.as_uuid())
            .bind(location_id.as_uuid())
            .bind(variant_id.as_uuid())
            .execute(&mut **tx)
            .await
            .map_err(database_error)?;
            (id, InventoryBalance::new(0, 0)?)
        }
    };
    let updated = current.adjust(quantity)?;
    sqlx::query(
        "UPDATE commerce.inventory_items SET on_hand_quantity = $2, updated_at = $3 WHERE id = $1",
    )
    .bind(stock_id)
    .bind(updated.on_hand())
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO commerce.inventory_transactions \
         (id, store_id, inventory_item_id, reference_type, reference_id, \
          on_hand_delta_quantity, \
          reserved_delta_quantity, resulting_on_hand_quantity, resulting_reserved_quantity, \
          note, actor_user_id, created_at) \
         VALUES ($1, $2, $3, 'return', $4, $5, 0, $6, $7, $8, $9, $10)",
    )
    .bind(Uuid::now_v7())
    .bind(store_id.as_uuid())
    .bind(stock_id)
    .bind(return_id.as_uuid())
    .bind(quantity)
    .bind(updated.on_hand())
    .bind(updated.reserved())
    .bind(format!("Return {} received", return_id.as_uuid()))
    .bind(actor_user_id)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}
