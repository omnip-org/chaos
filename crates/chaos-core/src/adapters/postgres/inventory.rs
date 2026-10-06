use chaos_domain::{catalog::ProductVariantId, inventory::InventoryBalance, store::StoreId};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApplicationError,
    contracts::{AdminActor, InventoryAdjustment, VariantInventoryView},
    error::database_error,
};

#[derive(sqlx::FromRow)]
struct VariantInventoryRow {
    id: Uuid,
    on_hand_quantity: i64,
    reserved_quantity: i64,
    updated_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct PostgresInventoryRepository {
    pool: PgPool,
}

impl PostgresInventoryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub(crate) async fn adjust_variant_inventory(
        &self,
        actor: AdminActor,
        adjustment: &InventoryAdjustment,
    ) -> Result<VariantInventoryView, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        require_store(&mut transaction, adjustment.store_id).await?;
        let (on_hand, reserved) = sqlx::query_as::<_, (i64, i64)>(
            "SELECT on_hand_quantity, reserved_quantity \
             FROM chaos_commerce.product_variants \
             WHERE store_id = $1 AND id = $2 AND track_inventory FOR UPDATE",
        )
        .bind(adjustment.store_id.as_uuid())
        .bind(adjustment.product_variant_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(invalid_inventory_selection)?;
        let next_on_hand =
            on_hand
                .checked_add(adjustment.delta_quantity)
                .ok_or(ApplicationError::Conflict {
                    code: "inventory_overflow",
                    message: "inventory arithmetic overflowed",
                })?;
        if next_on_hand < reserved {
            return Err(ApplicationError::Conflict {
                code: "inventory_reserved",
                message: "on-hand inventory cannot be reduced below active reservations",
            });
        }
        let balance = InventoryBalance::new(on_hand)
            .map_err(ApplicationError::from)?
            .adjust(adjustment.delta_quantity)
            .map_err(ApplicationError::from)?;
        let updated_at: OffsetDateTime = sqlx::query_scalar(
            "UPDATE chaos_commerce.product_variants \
             SET on_hand_quantity = $3, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2 RETURNING updated_at",
        )
        .bind(adjustment.store_id.as_uuid())
        .bind(adjustment.product_variant_id.as_uuid())
        .bind(balance.on_hand())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let inventory = VariantInventoryView {
            product_variant_id: adjustment.product_variant_id,
            on_hand_quantity: balance.on_hand(),
            reserved_quantity: reserved,
            updated_at,
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(inventory)
    }

    pub(crate) async fn list_variant_inventory(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<ProductVariantId>,
        limit: u16,
    ) -> Result<Option<Vec<VariantInventoryView>>, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        if !store_exists(&mut transaction, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, VariantInventoryRow>(
            "SELECT id, on_hand_quantity, reserved_quantity, updated_at \
             FROM chaos_commerce.product_variants \
             WHERE store_id = $1 AND track_inventory \
               AND ($2::uuid IS NULL OR id > $2) \
             ORDER BY id ASC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(ProductVariantId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(rows.into_iter().map(variant_inventory).collect()))
    }

    async fn begin_for_admin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_admin_context(
            &mut transaction,
            actor.audit_user_id(),
            actor.store_id(),
        )
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }
}

fn variant_inventory(row: VariantInventoryRow) -> VariantInventoryView {
    VariantInventoryView {
        product_variant_id: ProductVariantId::from_uuid(row.id),
        on_hand_quantity: row.on_hand_quantity,
        reserved_quantity: row.reserved_quantity,
        updated_at: row.updated_at,
    }
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
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM chaos_commerce.stores WHERE id = $1)")
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
