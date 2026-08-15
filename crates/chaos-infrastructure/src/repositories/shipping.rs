use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{IdempotencyRequest, ShippingServiceDetail, ShippingServiceRepository},
};
use chaos_domain::{
    CurrencyCode,
    fulfillment::{ShippingService, ShippingServiceId, ShippingServiceStatus},
    merchant::StoreId,
    pricing::Money,
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_OPERATION: &str = "shipping_services.create.v1";
const ACTIVATE_OPERATION: &str = "shipping_services.activate.v1";
const ARCHIVE_OPERATION: &str = "shipping_services.archive.v1";

type ServiceRow = (
    Uuid,
    String,
    String,
    i64,
    String,
    i16,
    i16,
    String,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(Clone)]
pub struct PostgresShippingServiceRepository {
    pool: PgPool,
}

impl PostgresShippingServiceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: MerchantActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.user_id().as_uuid().to_string())
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
impl ShippingServiceRepository for PostgresShippingServiceRepository {
    async fn create_shipping_service(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        service: &ShippingService,
        request: &IdempotencyRequest,
    ) -> Result<ShippingServiceDetail, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(&mut transaction, actor, CREATE_OPERATION, request).await? {
            let detail = load(
                &mut transaction,
                actor,
                store_id,
                ShippingServiceId::from_uuid(id),
            )
            .await?
            .ok_or_else(|| service_not_found(ShippingServiceId::from_uuid(id)))?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(detail);
        }
        require_store_currency(&mut transaction, actor, store_id, service.rate().currency())
            .await?;
        sqlx::query(
            "INSERT INTO fulfillment.shipping_services \
             (id, merchant_account_id, store_id, code, name, amount_minor, currency, \
              estimated_min_days, estimated_max_days, status) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::fulfillment.shipping_service_status)",
        )
        .bind(service.id().as_uuid())
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(service.code())
        .bind(service.name())
        .bind(service.rate().amount_minor())
        .bind(service.rate().currency().as_str())
        .bind(i16::try_from(service.estimated_min_days()).map_err(conversion_error)?)
        .bind(i16::try_from(service.estimated_max_days()).map_err(conversion_error)?)
        .bind(service.status().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_create_error)?;
        for country in service.destination_countries() {
            sqlx::query(
                "INSERT INTO fulfillment.shipping_service_regions \
                 (merchant_account_id, store_id, shipping_service_id, country_code) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(actor.merchant_account_id().as_uuid())
            .bind(store_id.as_uuid())
            .bind(service.id().as_uuid())
            .bind(country)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        complete(
            &mut transaction,
            actor,
            CREATE_OPERATION,
            request,
            service.id(),
        )
        .await?;
        let detail = load(&mut transaction, actor, store_id, service.id())
            .await?
            .ok_or_else(|| service_not_found(service.id()))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn list_shipping_services(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingServiceDetail>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        require_store(&mut transaction, actor, store_id).await?;
        let rows = sqlx::query_as::<_, ServiceRow>(
            "SELECT id, code, name, amount_minor, currency::text, estimated_min_days, \
                    estimated_max_days, status::text, created_at, updated_at \
             FROM fulfillment.shipping_services \
             WHERE merchant_account_id = $1 AND store_id = $2 ORDER BY created_at, id",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let mut details = Vec::with_capacity(rows.len());
        for row in rows {
            details.push(detail(&mut transaction, actor, store_id, row).await?);
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(details)
    }

    async fn change_shipping_service_status(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        service_id: ShippingServiceId,
        status: ShippingServiceStatus,
        request: &IdempotencyRequest,
    ) -> Result<ShippingServiceDetail, ApplicationError> {
        let operation = match status {
            ShippingServiceStatus::Active => ACTIVATE_OPERATION,
            ShippingServiceStatus::Archived => ARCHIVE_OPERATION,
        };
        let mut transaction = self.begin(actor).await?;
        if reserve(&mut transaction, actor, operation, request)
            .await?
            .is_none()
        {
            let result = sqlx::query(
                "UPDATE fulfillment.shipping_services SET status = $4::fulfillment.shipping_service_status, \
                        updated_at = CURRENT_TIMESTAMP \
                 WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
            )
            .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid())
            .bind(service_id.as_uuid()).bind(status.as_str())
            .execute(&mut *transaction).await.map_err(database_error)?;
            if result.rows_affected() != 1 {
                return Err(service_not_found(service_id));
            }
            complete(&mut transaction, actor, operation, request, service_id).await?;
        }
        let detail = load(&mut transaction, actor, store_id, service_id)
            .await?
            .ok_or_else(|| service_not_found(service_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
}

async fn load(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    store_id: StoreId,
    service_id: ShippingServiceId,
) -> Result<Option<ShippingServiceDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, ServiceRow>(
        "SELECT id, code, name, amount_minor, currency::text, estimated_min_days, \
                estimated_max_days, status::text, created_at, updated_at \
         FROM fulfillment.shipping_services \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .bind(service_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    match row {
        Some(row) => Ok(Some(detail(transaction, actor, store_id, row).await?)),
        None => Ok(None),
    }
}

async fn detail(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    store_id: StoreId,
    row: ServiceRow,
) -> Result<ShippingServiceDetail, ApplicationError> {
    let countries = sqlx::query_scalar::<_, String>(
        "SELECT country_code::text FROM fulfillment.shipping_service_regions \
         WHERE merchant_account_id = $1 AND store_id = $2 AND shipping_service_id = $3 \
         ORDER BY country_code",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .bind(row.0)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let min = u16::try_from(row.5).map_err(conversion_error)?;
    let max = u16::try_from(row.6).map_err(conversion_error)?;
    let service = ShippingService::rehydrate(
        ShippingServiceId::from_uuid(row.0),
        row.1,
        row.2,
        Money::new(row.3, CurrencyCode::parse(&row.4)?),
        min,
        max,
        countries,
        ShippingServiceStatus::parse(&row.7).ok_or_else(corrupt_state)?,
    )?;
    Ok(ShippingServiceDetail {
        service,
        created_at: row.8,
        updated_at: row.9,
    })
}

async fn require_store_currency(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    store_id: StoreId,
    currency: CurrencyCode,
) -> Result<(), ApplicationError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM merchant.stores s \
         JOIN merchant.store_currencies c ON c.merchant_account_id = s.merchant_account_id \
          AND c.store_id = s.id \
         WHERE s.merchant_account_id = $1 AND s.id = $2 AND s.status <> 'archived' \
           AND c.currency = $3 AND c.enabled)",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .bind(currency.as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if exists {
        Ok(())
    } else {
        Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "currency",
                reason: "must be enabled for the Store".into(),
            }],
        })
    }
}

async fn require_store(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM merchant.stores WHERE merchant_account_id = $1 AND id = $2)",
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
    actor: MerchantActor,
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
    actor: MerchantActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: ShippingServiceId,
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

fn service_not_found(id: ShippingServiceId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "shipping_service",
        id: id.as_uuid().to_string(),
    }
}
fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Shipping Service state"
    ))
}
fn conversion_error(error: std::num::TryFromIntError) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
fn map_create_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(error) = &error
        && error.constraint() == Some("shipping_services_merchant_account_id_store_id_code_key")
    {
        return ApplicationError::Conflict {
            code: "shipping_service_code_taken",
            message: "the Shipping Service code is already in use for this Store",
        };
    }
    database_error(error)
}
