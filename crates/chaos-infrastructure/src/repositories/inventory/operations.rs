use serde_json::Value;

use crate::repositories::shared::idempotency::{self, IdempotencyScope};

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

fn invalid_inventory_selection() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "product_variant_id",
            reason: "must reference an inventory-tracked variant in the Store".into(),
        }],
    }
}

async fn reserve_idempotency(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: Uuid,
    operation: &'static str,
    request: &chaos_application::ports::IdempotencyRequest,
) -> Result<Option<Value>, ApplicationError> {
    idempotency::reserve(
        transaction,
        &IdempotencyScope::Store(store_id),
        operation,
        request,
    )
    .await
}

async fn complete_snapshot(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: Uuid,
    operation: &'static str,
    request: &chaos_application::ports::IdempotencyRequest,
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
