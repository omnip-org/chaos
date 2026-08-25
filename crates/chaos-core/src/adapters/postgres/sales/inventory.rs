use crate::{ApplicationError, error::database_error};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(crate) async fn consume_order_inventory(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: Uuid,
    order_id: Uuid,
) -> Result<(), ApplicationError> {
    let lines = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT product_variant_id, quantity::bigint \
         FROM commerce.order_lines \
         WHERE store_id = $1 AND order_id = $2 AND track_inventory \
         ORDER BY product_variant_id",
    )
    .bind(store_id)
    .bind(order_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;

    for (product_variant_id, quantity) in lines {
        sqlx::query_scalar::<_, i64>(
            "UPDATE commerce.product_variants \
             SET on_hand_quantity = on_hand_quantity - $3, \
                 reserved_quantity = GREATEST(reserved_quantity - $3, 0), \
                 updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2 AND track_inventory \
               AND on_hand_quantity >= $3 \
             RETURNING on_hand_quantity",
        )
        .bind(store_id)
        .bind(product_variant_id)
        .bind(quantity)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ApplicationError::Conflict {
            code: "insufficient_inventory",
            message: "one or more order lines exceed available inventory",
        })?;
    }
    Ok(())
}

pub(crate) async fn release_order_inventory(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: Uuid,
    order_id: Uuid,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE commerce.product_variants AS variant \
         SET reserved_quantity = variant.reserved_quantity - LEAST(variant.reserved_quantity, lines.quantity), \
             updated_at = CURRENT_TIMESTAMP \
         FROM ( \
             SELECT product_variant_id, SUM(quantity)::bigint AS quantity \
             FROM commerce.order_lines \
             WHERE store_id = $1 AND order_id = $2 AND track_inventory \
             GROUP BY product_variant_id \
         ) AS lines \
         WHERE variant.store_id = $1 AND variant.id = lines.product_variant_id",
    )
    .bind(store_id)
    .bind(order_id)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}
