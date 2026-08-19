use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{AdminActor, IdempotencyRequest, TaxRuleDetail, TaxRuleRepository},
};
use chaos_domain::{
    merchant::StoreId,
    pricing::{TaxRule, TaxRuleId, TaxRuleStatus},
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_OPERATION: &str = "tax_rules.create.v1";
const ACTIVATE_OPERATION: &str = "tax_rules.activate.v1";
const ARCHIVE_OPERATION: &str = "tax_rules.archive.v1";

type TaxRuleRow = (
    Uuid,
    String,
    String,
    String,
    i32,
    String,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(Clone)]
pub struct PostgresTaxRuleRepository {
    pool: PgPool,
}

impl PostgresTaxRuleRepository {
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
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(actor.store_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

#[async_trait]
impl TaxRuleRepository for PostgresTaxRuleRepository {
    async fn create_tax_rule(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        rule: &TaxRule,
        request: &IdempotencyRequest,
    ) -> Result<TaxRuleDetail, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut transaction, &actor, CREATE_OPERATION, request).await? {
            let detail = load(&mut transaction, store_id, TaxRuleId::from_uuid(id))
                .await?
                .ok_or_else(|| rule_not_found(TaxRuleId::from_uuid(id)))?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(detail);
        }
        require_store(&mut transaction, store_id).await?;
        sqlx::query(
            "INSERT INTO pricing.tax_rules \
             (id, store_id, code, name, country_code, rate_basis_points, status) \
             VALUES ($1, $2, $3, $4, $5, $6, $7::pricing.tax_rule_status)",
        )
        .bind(rule.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(rule.code())
        .bind(rule.name())
        .bind(rule.country_code())
        .bind(i32::try_from(rule.rate_basis_points()).map_err(conversion_error)?)
        .bind(rule.status().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_write_error)?;
        complete(
            &mut transaction,
            &actor,
            CREATE_OPERATION,
            request,
            rule.id(),
        )
        .await?;
        let detail = load(&mut transaction, store_id, rule.id())
            .await?
            .ok_or_else(|| rule_not_found(rule.id()))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn list_tax_rules(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<TaxRuleDetail>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        require_store(&mut transaction, store_id).await?;
        let rows = sqlx::query_as::<_, TaxRuleRow>(
            "SELECT id, code, name, country_code::text, rate_basis_points, status::text, \
                    created_at, updated_at FROM pricing.tax_rules \
             WHERE store_id = $1 ORDER BY created_at, id",
        )
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter().map(tax_rule_detail).collect()
    }

    async fn change_tax_rule_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        rule_id: TaxRuleId,
        status: TaxRuleStatus,
        request: &IdempotencyRequest,
    ) -> Result<TaxRuleDetail, ApplicationError> {
        let operation = match status {
            TaxRuleStatus::Active => ACTIVATE_OPERATION,
            TaxRuleStatus::Archived => ARCHIVE_OPERATION,
        };
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, operation, request)
            .await?
            .is_none()
        {
            let result = sqlx::query(
                "UPDATE pricing.tax_rules SET status = $3::pricing.tax_rule_status, updated_at = CURRENT_TIMESTAMP \
                 WHERE store_id = $1 AND id = $2",
            ).bind(store_id.as_uuid())
              .bind(rule_id.as_uuid()).bind(status.as_str())
              .execute(&mut *transaction).await.map_err(map_write_error)?;
            if result.rows_affected() != 1 {
                return Err(rule_not_found(rule_id));
            }
            complete(&mut transaction, &actor, operation, request, rule_id).await?;
        }
        let detail = load(&mut transaction, store_id, rule_id)
            .await?
            .ok_or_else(|| rule_not_found(rule_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
}

async fn load(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    rule_id: TaxRuleId,
) -> Result<Option<TaxRuleDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, TaxRuleRow>(
        "SELECT id, code, name, country_code::text, rate_basis_points, status::text, \
                created_at, updated_at FROM pricing.tax_rules \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(rule_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(tax_rule_detail).transpose()
}

fn tax_rule_detail(row: TaxRuleRow) -> Result<TaxRuleDetail, ApplicationError> {
    Ok(TaxRuleDetail {
        rule: TaxRule::rehydrate(
            TaxRuleId::from_uuid(row.0),
            row.1,
            row.2,
            row.3,
            u32::try_from(row.4).map_err(conversion_error)?,
            TaxRuleStatus::parse(&row.5).ok_or_else(corrupt_state)?,
        )?,
        created_at: row.6,
        updated_at: row.7,
    })
}

async fn require_store(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM merchant.stores WHERE id = $1)")
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
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
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
    id: TaxRuleId,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
        operation,
        request,
        200,
        json!({ "data": { "id": id.as_uuid() } }),
    )
    .await
}

fn map_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(error) = &error {
        match error.constraint() {
            Some("tax_rules_store_id_code_key") => {
                return ApplicationError::Conflict {
                    code: "tax_rule_code_taken",
                    message: "the Tax Rule code is already in use for this Store",
                };
            }
            Some("tax_rules_active_country_key") => {
                return ApplicationError::Conflict {
                    code: "active_tax_rule_exists",
                    message: "an active Tax Rule already exists for this Store and country",
                };
            }
            _ => {}
        }
    }
    database_error(error)
}

fn rule_not_found(id: TaxRuleId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "tax_rule",
        id: id.as_uuid().to_string(),
    }
}
fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains invalid Tax Rule state"))
}
fn conversion_error(error: std::num::TryFromIntError) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
