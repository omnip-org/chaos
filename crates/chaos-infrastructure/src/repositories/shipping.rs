use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        IdempotencyRequest, ShippingAddress, ShippingProviderAccountConfiguration,
        ShippingProviderAccountDetail, ShippingProviderAccountRepository, ShippingServiceDetail,
        ShippingServiceRepository,
    },
};
use chaos_domain::{
    CurrencyCode,
    fulfillment::{
        ShippingProviderAccount, ShippingProviderAccountId, ShippingService, ShippingServiceId,
        ShippingServiceStatus,
    },
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
const CREATE_PROVIDER_ACCOUNT_OPERATION: &str = "shipping_provider_accounts.create.v1";
const UPDATE_PROVIDER_ACCOUNT_OPERATION: &str = "shipping_provider_accounts.update.v1";

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

#[derive(sqlx::FromRow)]
struct ProviderAccountRow {
    id: Uuid,
    provider: String,
    display_name: String,
    enabled: bool,
    credentials_configured: bool,
    origin_name: String,
    origin_company: Option<String>,
    origin_address_line_1: String,
    origin_address_line_2: Option<String>,
    origin_city: String,
    origin_region: Option<String>,
    origin_postal_code: String,
    origin_country_code: String,
    origin_phone: Option<String>,
    origin_email: Option<String>,
    credential_rotation_expires_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

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

#[async_trait]
impl ShippingProviderAccountRepository for PostgresShippingServiceRepository {
    async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingProviderAccountDetail>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        require_store(&mut transaction, actor, store_id).await?;
        let rows = sqlx::query_as::<_, ProviderAccountRow>(
            "SELECT id, provider, display_name, enabled, \
                    credential_secret_reference IS NOT NULL AS credentials_configured, \
                    origin_name, origin_company, origin_address_line_1, origin_address_line_2, \
                    origin_city, origin_region, origin_postal_code, origin_country_code::text, \
                    origin_phone, origin_email, credential_rotation_expires_at, created_at, updated_at \
             FROM fulfillment.shipping_provider_accounts \
             WHERE merchant_account_id = $1 AND store_id = $2 ORDER BY created_at, id",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let values = rows
            .into_iter()
            .map(provider_account_detail)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(database_error)?;
        Ok(values)
    }

    async fn get(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        id: ShippingProviderAccountId,
    ) -> Result<Option<ShippingProviderAccountDetail>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let value = load_provider_account(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            store_id,
            id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn create(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        account: &ShippingProviderAccount,
        configuration: &ShippingProviderAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(
            &mut transaction,
            actor,
            CREATE_PROVIDER_ACCOUNT_OPERATION,
            request,
        )
        .await?
        {
            let value = load_provider_account(
                &mut transaction,
                account_id,
                store_id,
                ShippingProviderAccountId::from_uuid(id),
            )
            .await?
            .ok_or_else(corrupt_provider_state)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(value);
        }
        let origin = &configuration.origin;
        sqlx::query(
            "INSERT INTO fulfillment.shipping_provider_accounts \
             (id, merchant_account_id, store_id, provider, display_name, credential_secret_reference, \
              origin_name, origin_company, origin_address_line_1, origin_address_line_2, origin_city, \
              origin_region, origin_postal_code, origin_country_code, origin_phone, origin_email, \
              enabled, created_by_user_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)",
        )
        .bind(account.id().as_uuid())
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(account.provider())
        .bind(account.display_name())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(&origin.name)
        .bind(&origin.company)
        .bind(&origin.address_line_1)
        .bind(&origin.address_line_2)
        .bind(&origin.city)
        .bind(&origin.region)
        .bind(&origin.postal_code)
        .bind(&origin.country_code)
        .bind(&origin.phone)
        .bind(&origin.email)
        .bind(account.enabled())
        .bind(actor.user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        complete_provider_account(
            &mut transaction,
            actor,
            CREATE_PROVIDER_ACCOUNT_OPERATION,
            request,
            account.id(),
        )
        .await?;
        let value = load_provider_account(&mut transaction, account_id, store_id, account.id())
            .await?
            .ok_or_else(corrupt_provider_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn update(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        account: &ShippingProviderAccount,
        configuration: &ShippingProviderAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<ShippingProviderAccountDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(
            &mut transaction,
            actor,
            UPDATE_PROVIDER_ACCOUNT_OPERATION,
            request,
        )
        .await?
        {
            let value = load_provider_account(
                &mut transaction,
                account_id,
                store_id,
                ShippingProviderAccountId::from_uuid(id),
            )
            .await?
            .ok_or_else(corrupt_provider_state)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(value);
        }
        let origin = &configuration.origin;
        let result = sqlx::query(
            "UPDATE fulfillment.shipping_provider_accounts SET display_name = $4, \
                    previous_credential_secret_reference = CASE \
                        WHEN credential_secret_reference IS DISTINCT FROM $5 \
                        THEN credential_secret_reference ELSE previous_credential_secret_reference END, \
                    credential_rotation_expires_at = CASE \
                        WHEN credential_secret_reference IS DISTINCT FROM $5 \
                        THEN CURRENT_TIMESTAMP + INTERVAL '24 hours' ELSE credential_rotation_expires_at END, \
                    credential_secret_reference = $5, origin_name = $6, origin_company = $7, \
                    origin_address_line_1 = $8, origin_address_line_2 = $9, origin_city = $10, \
                    origin_region = $11, origin_postal_code = $12, origin_country_code = $13, \
                    origin_phone = $14, origin_email = $15, enabled = $16, updated_at = CURRENT_TIMESTAMP \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(account.id().as_uuid())
        .bind(account.display_name())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(&origin.name)
        .bind(&origin.company)
        .bind(&origin.address_line_1)
        .bind(&origin.address_line_2)
        .bind(&origin.city)
        .bind(&origin.region)
        .bind(&origin.postal_code)
        .bind(&origin.country_code)
        .bind(&origin.phone)
        .bind(&origin.email)
        .bind(account.enabled())
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        if result.rows_affected() != 1 {
            return Err(provider_account_not_found(account.id()));
        }
        complete_provider_account(
            &mut transaction,
            actor,
            UPDATE_PROVIDER_ACCOUNT_OPERATION,
            request,
            account.id(),
        )
        .await?;
        let value = load_provider_account(&mut transaction, account_id, store_id, account.id())
            .await?
            .ok_or_else(corrupt_provider_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }
}

async fn load_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    id: ShippingProviderAccountId,
) -> Result<Option<ShippingProviderAccountDetail>, ApplicationError> {
    sqlx::query_as::<_, ProviderAccountRow>(
        "SELECT id, provider, display_name, enabled, \
                credential_secret_reference IS NOT NULL AS credentials_configured, \
                origin_name, origin_company, origin_address_line_1, origin_address_line_2, \
                origin_city, origin_region, origin_postal_code, origin_country_code::text, \
                origin_phone, origin_email, credential_rotation_expires_at, created_at, updated_at \
         FROM fulfillment.shipping_provider_accounts \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(provider_account_detail)
    .transpose()
}

fn provider_account_detail(
    row: ProviderAccountRow,
) -> Result<ShippingProviderAccountDetail, ApplicationError> {
    Ok(ShippingProviderAccountDetail {
        account: ShippingProviderAccount::rehydrate(
            ShippingProviderAccountId::from_uuid(row.id),
            row.provider,
            row.display_name,
            row.enabled,
        )?,
        credentials_configured: row.credentials_configured,
        origin: ShippingAddress {
            name: row.origin_name,
            company: row.origin_company,
            address_line_1: row.origin_address_line_1,
            address_line_2: row.origin_address_line_2,
            city: row.origin_city,
            region: row.origin_region,
            postal_code: row.origin_postal_code,
            country_code: row.origin_country_code,
            phone: row.origin_phone,
            email: row.origin_email,
        },
        credential_rotation_expires_at: row.credential_rotation_expires_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
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

async fn complete_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: ShippingProviderAccountId,
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

fn provider_account_not_found(id: ShippingProviderAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "shipping_provider_account",
        id: id.as_uuid().to_string(),
    }
}

fn corrupt_provider_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Shipping Provider Account state"
    ))
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

fn map_provider_account_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(error) = &error
        && error.constraint() == Some("shipping_provider_accounts_store_provider_key")
    {
        return ApplicationError::Conflict {
            code: "shipping_provider_already_configured",
            message: "the Shipping Provider is already configured for this Store",
        };
    }
    database_error(error)
}
