use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{AdminActor, IdempotencyRequest, PromotionDetail, PromotionRepository},
};
use chaos_domain::{
    CurrencyCode,
    merchant::StoreId,
    pricing::{Promotion, PromotionId, PromotionStatus, PromotionTrigger, PromotionValue},
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_OPERATION: &str = "promotions.create.v1";
const ACTIVATE_OPERATION: &str = "promotions.activate.v1";
const ARCHIVE_OPERATION: &str = "promotions.archive.v1";

#[derive(sqlx::FromRow)]
struct PromotionRow {
    id: Uuid,
    handle: String,
    name: String,
    trigger: String,
    redemption_code: Option<String>,
    value_kind: String,
    rate_basis_points: Option<i32>,
    amount_minor: Option<i64>,
    maximum_amount_minor: Option<i64>,
    currency: String,
    minimum_subtotal_amount_minor: i64,
    priority: i16,
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
    status: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct PostgresPromotionRepository {
    pool: PgPool,
}

impl PostgresPromotionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    async fn begin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(actor.merchant_account_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

#[async_trait]
impl PromotionRepository for PostgresPromotionRepository {
    async fn create_promotion(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        promotion: &Promotion,
        request: &IdempotencyRequest,
    ) -> Result<PromotionDetail, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut transaction, &actor, CREATE_OPERATION, request).await? {
            let detail = load(
                &mut transaction,
                &actor,
                store_id,
                PromotionId::from_uuid(id),
            )
            .await?
            .ok_or_else(|| not_found(PromotionId::from_uuid(id)))?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(detail);
        }
        require_store(&mut transaction, &actor, store_id).await?;
        sqlx::query("INSERT INTO pricing.promotions \
            (id, merchant_account_id, store_id, handle, name, trigger, redemption_code, value_kind, \
             rate_basis_points, amount_minor, maximum_amount_minor, currency, minimum_subtotal_amount_minor, \
             priority, starts_at, ends_at, status) \
            VALUES ($1,$2,$3,$4,$5,$6::pricing.promotion_trigger,$7,$8::pricing.promotion_value_kind, \
                    $9,$10,$11,$12,$13,$14,$15,$16,$17::pricing.promotion_status)")
            .bind(promotion.id().as_uuid()).bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid())
            .bind(promotion.handle()).bind(promotion.name()).bind(promotion.trigger().as_str())
            .bind(promotion.redemption_code()).bind(promotion.value().kind())
            .bind(promotion.value().rate_basis_points().map(i32::try_from).transpose().map_err(conversion_error)?)
            .bind(promotion.value().amount_minor()).bind(promotion.value().maximum_amount_minor())
            .bind(promotion.currency().as_str()).bind(promotion.minimum_subtotal_amount_minor())
            .bind(i16::try_from(promotion.priority()).map_err(conversion_error)?)
            .bind(promotion.starts_at()).bind(promotion.ends_at()).bind(promotion.status().as_str())
            .execute(&mut *transaction).await.map_err(map_write_error)?;
        complete(
            &mut transaction,
            &actor,
            CREATE_OPERATION,
            request,
            promotion.id(),
        )
        .await?;
        let detail = load(&mut transaction, &actor, store_id, promotion.id())
            .await?
            .ok_or_else(|| not_found(promotion.id()))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn list_promotions(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<PromotionDetail>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        require_store(&mut transaction, &actor, store_id).await?;
        let rows = sqlx::query_as::<_, PromotionRow>(
            "SELECT id, handle, name, trigger::text, redemption_code::text, value_kind::text, \
             rate_basis_points, amount_minor, maximum_amount_minor, currency::text, \
             minimum_subtotal_amount_minor, priority, starts_at, ends_at, status::text, \
             created_at, updated_at FROM pricing.promotions \
             WHERE merchant_account_id = $1 AND store_id = $2 ORDER BY created_at, id",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter().map(detail).collect()
    }

    async fn change_promotion_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        promotion_id: PromotionId,
        status: PromotionStatus,
        request: &IdempotencyRequest,
    ) -> Result<PromotionDetail, ApplicationError> {
        let operation = match status {
            PromotionStatus::Active => ACTIVATE_OPERATION,
            PromotionStatus::Archived => ARCHIVE_OPERATION,
        };
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, operation, request)
            .await?
            .is_none()
        {
            let result = sqlx::query("UPDATE pricing.promotions SET status = $4::pricing.promotion_status, \
                updated_at = CURRENT_TIMESTAMP WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3")
                .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid())
                .bind(promotion_id.as_uuid()).bind(status.as_str()).execute(&mut *transaction).await.map_err(map_write_error)?;
            if result.rows_affected() != 1 {
                return Err(not_found(promotion_id));
            }
            complete(&mut transaction, &actor, operation, request, promotion_id).await?;
        }
        let value = load(&mut transaction, &actor, store_id, promotion_id)
            .await?
            .ok_or_else(|| not_found(promotion_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }
}

async fn load(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    store_id: StoreId,
    id: PromotionId,
) -> Result<Option<PromotionDetail>, ApplicationError> {
    sqlx::query_as::<_, PromotionRow>(
        "SELECT id, handle, name, trigger::text, redemption_code::text, value_kind::text, \
         rate_basis_points, amount_minor, maximum_amount_minor, currency::text, \
         minimum_subtotal_amount_minor, priority, starts_at, ends_at, status::text, \
         created_at, updated_at FROM pricing.promotions \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(detail)
    .transpose()
}

fn detail(row: PromotionRow) -> Result<PromotionDetail, ApplicationError> {
    let value = match row.value_kind.as_str() {
        "percentage" => PromotionValue::Percentage {
            rate_basis_points: u32::try_from(row.rate_basis_points.ok_or_else(corrupt_state)?)
                .map_err(conversion_error)?,
            maximum_amount_minor: row.maximum_amount_minor,
        },
        "fixed_amount" => PromotionValue::FixedAmount {
            amount_minor: row.amount_minor.ok_or_else(corrupt_state)?,
        },
        _ => return Err(corrupt_state()),
    };
    Ok(PromotionDetail {
        promotion: Promotion::rehydrate(
            PromotionId::from_uuid(row.id),
            row.handle,
            row.name,
            PromotionTrigger::parse(&row.trigger).ok_or_else(corrupt_state)?,
            row.redemption_code,
            value,
            CurrencyCode::parse(&row.currency)?,
            row.minimum_subtotal_amount_minor,
            u16::try_from(row.priority).map_err(conversion_error)?,
            row.starts_at,
            row.ends_at,
            PromotionStatus::parse(&row.status).ok_or_else(corrupt_state)?,
        )?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn require_store(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM merchant.stores \
        WHERE merchant_account_id = $1 AND id = $2 AND status <> 'archived')",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if exists {
        Ok(())
    } else {
        Err(ApplicationError::NotFound {
            resource: "store",
            id: store_id.as_uuid().to_string(),
        })
    }
}

async fn reserve(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(body) = idempotency::reserve(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
    )
    .await?
    else {
        return Ok(None);
    };
    body.pointer("/data/id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(Some)
        .ok_or_else(corrupt_state)
}

async fn complete(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: PromotionId,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
        200,
        json!({ "data": { "id": id.as_uuid() } }),
    )
    .await
}

fn map_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error {
        match database.constraint() {
            Some("promotions_merchant_account_id_store_id_handle_key") => {
                return ApplicationError::Conflict {
                    code: "promotion_handle_taken",
                    message: "the Promotion handle is already in use for this Store",
                };
            }
            Some("promotions_active_redemption_code_key") => {
                return ApplicationError::Conflict {
                    code: "promotion_code_taken",
                    message: "the Promotion redemption code is already in use for this Store",
                };
            }
            _ => {}
        }
    }
    database_error(error)
}
fn not_found(id: PromotionId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "promotion",
        id: id.as_uuid().to_string(),
    }
}
fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains invalid Promotion state"))
}
fn conversion_error(error: std::num::TryFromIntError) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
