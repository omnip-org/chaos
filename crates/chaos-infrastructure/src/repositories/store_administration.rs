use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        IdempotencyRequest, SalesChannelAdminItem, StoreAdminItem, StoreAdministrationRepository,
    },
};
use chaos_domain::{
    CurrencyCode, RegionCode,
    merchant::{
        SalesChannel, SalesChannelCode, SalesChannelId, SalesChannelKind, SalesChannelStatus,
        Store, StoreCode, StoreId, StoreStatus,
    },
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const UPDATE_STORE_OPERATION: &str = "stores.update.v1";
const ACTIVATE_STORE_OPERATION: &str = "stores.activate.v1";
const ARCHIVE_STORE_OPERATION: &str = "stores.archive.v1";
const CREATE_CHANNEL_OPERATION: &str = "sales_channels.create.v1";
const UPDATE_CHANNEL_OPERATION: &str = "sales_channels.update.v1";
const ACTIVATE_CHANNEL_OPERATION: &str = "sales_channels.activate.v1";
const ARCHIVE_CHANNEL_OPERATION: &str = "sales_channels.archive.v1";

#[derive(Clone)]
pub struct PostgresStoreAdministrationRepository {
    pool: PgPool,
}

impl PostgresStoreAdministrationRepository {
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

type StoreRow = (
    Uuid,
    String,
    String,
    String,
    String,
    String,
    OffsetDateTime,
    OffsetDateTime,
);

type ChannelRow = (
    Uuid,
    String,
    String,
    String,
    String,
    bool,
    OffsetDateTime,
    OffsetDateTime,
);

#[async_trait]
impl StoreAdministrationRepository for PostgresStoreAdministrationRepository {
    async fn get_store(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Option<StoreAdminItem>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let row = sqlx::query_as::<_, StoreRow>(
            "SELECT id, code::text, name, default_region::text, default_currency::text, \
                    status::text, created_at, updated_at \
             FROM merchant.stores WHERE merchant_account_id = $1 AND id = $2",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        row.map(store_item).transpose()
    }

    async fn update_store(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        replacement: &Store,
        request: &IdempotencyRequest,
    ) -> Result<StoreId, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(&mut transaction, actor, UPDATE_STORE_OPERATION, request).await? {
            return Ok(StoreId::from_uuid(id));
        }
        let result = sqlx::query(
            "UPDATE merchant.stores SET code = $3, name = $4, default_region = $5, \
                    default_currency = $6, updated_at = CURRENT_TIMESTAMP \
             WHERE merchant_account_id = $1 AND id = $2",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(replacement.code().as_str())
        .bind(replacement.name())
        .bind(replacement.default_region().as_str())
        .bind(replacement.default_currency().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_store_error)?;
        if result.rows_affected() == 0 {
            return Err(store_not_found(store_id));
        }
        sqlx::query(
            "INSERT INTO merchant.store_currencies \
             (merchant_account_id, store_id, currency, enabled) VALUES ($1, $2, $3, true) \
             ON CONFLICT (merchant_account_id, store_id, currency) \
             DO UPDATE SET enabled = true",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(replacement.default_currency().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        complete(
            &mut transaction,
            actor,
            UPDATE_STORE_OPERATION,
            request,
            store_id.as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(store_id)
    }

    async fn change_store_status(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        status: StoreStatus,
        request: &IdempotencyRequest,
    ) -> Result<StoreId, ApplicationError> {
        let operation = match status {
            StoreStatus::Active => ACTIVATE_STORE_OPERATION,
            StoreStatus::Archived => ARCHIVE_STORE_OPERATION,
            StoreStatus::Draft => {
                return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                    "draft is not a Store lifecycle action"
                )));
            }
        };
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(&mut transaction, actor, operation, request).await? {
            return Ok(StoreId::from_uuid(id));
        }
        let default_currency = sqlx::query_scalar::<_, String>(
            "SELECT default_currency::text FROM merchant.stores \
             WHERE merchant_account_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| store_not_found(store_id))?;
        if status == StoreStatus::Active {
            let currency_enabled: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM merchant.store_currencies \
                 WHERE merchant_account_id = $1 AND store_id = $2 \
                   AND currency = $3 AND enabled)",
            )
            .bind(actor.merchant_account_id().as_uuid())
            .bind(store_id.as_uuid())
            .bind(&default_currency)
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            let active_default_channel: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM merchant.sales_channels \
                 WHERE merchant_account_id = $1 AND store_id = $2 \
                   AND is_default AND status = 'active')",
            )
            .bind(actor.merchant_account_id().as_uuid())
            .bind(store_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            Store::validate_activation(currency_enabled, active_default_channel)?;
        }
        sqlx::query(
            "UPDATE merchant.stores SET status = $3::merchant.store_status, \
                    updated_at = CURRENT_TIMESTAMP \
             WHERE merchant_account_id = $1 AND id = $2",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(status.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        complete(
            &mut transaction,
            actor,
            operation,
            request,
            store_id.as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(store_id)
    }

    async fn list_sales_channels(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<SalesChannelId>,
        limit: u16,
    ) -> Result<Option<Vec<SalesChannelAdminItem>>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if !store_exists(&mut transaction, actor, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, code::text, name, kind::text, status::text, is_default, \
                    created_at, updated_at FROM merchant.sales_channels \
             WHERE merchant_account_id = $1 AND store_id = $2 \
               AND ($3::uuid IS NULL OR id > $3) ORDER BY id ASC LIMIT $4",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(after.map(SalesChannelId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(channel_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn get_sales_channel(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
    ) -> Result<Option<SalesChannelAdminItem>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let row = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, code::text, name, kind::text, status::text, is_default, \
                    created_at, updated_at FROM merchant.sales_channels \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        row.map(channel_item).transpose()
    }

    async fn create_sales_channel(
        &self,
        actor: MerchantActor,
        channel: &SalesChannel,
        request: &IdempotencyRequest,
    ) -> Result<SalesChannelId, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if let Some(id) =
            reserve(&mut transaction, actor, CREATE_CHANNEL_OPERATION, request).await?
        {
            return Ok(SalesChannelId::from_uuid(id));
        }
        require_writable_store(&mut transaction, actor, channel.store_id()).await?;
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, merchant_account_id, store_id, code, name, kind, status, is_default) \
             VALUES ($1, $2, $3, $4, $5, $6::merchant.sales_channel_kind, 'active', false)",
        )
        .bind(channel.id().as_uuid())
        .bind(actor.merchant_account_id().as_uuid())
        .bind(channel.store_id().as_uuid())
        .bind(channel.code().as_str())
        .bind(channel.name())
        .bind(channel.kind().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_channel_error)?;
        complete(
            &mut transaction,
            actor,
            CREATE_CHANNEL_OPERATION,
            request,
            channel.id().as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(channel.id())
    }

    async fn update_sales_channel(
        &self,
        actor: MerchantActor,
        sales_channel_id: SalesChannelId,
        replacement: &SalesChannel,
        request: &IdempotencyRequest,
    ) -> Result<SalesChannelId, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if let Some(id) =
            reserve(&mut transaction, actor, UPDATE_CHANNEL_OPERATION, request).await?
        {
            return Ok(SalesChannelId::from_uuid(id));
        }
        let result = sqlx::query(
            "UPDATE merchant.sales_channels SET code = $4, name = $5, \
                    kind = $6::merchant.sales_channel_kind, updated_at = CURRENT_TIMESTAMP \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(replacement.store_id().as_uuid())
        .bind(sales_channel_id.as_uuid())
        .bind(replacement.code().as_str())
        .bind(replacement.name())
        .bind(replacement.kind().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_channel_error)?;
        if result.rows_affected() == 0 {
            return Err(channel_not_found(sales_channel_id));
        }
        complete(
            &mut transaction,
            actor,
            UPDATE_CHANNEL_OPERATION,
            request,
            sales_channel_id.as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(sales_channel_id)
    }

    async fn change_sales_channel_status(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        status: SalesChannelStatus,
        request: &IdempotencyRequest,
    ) -> Result<SalesChannelId, ApplicationError> {
        let operation = match status {
            SalesChannelStatus::Active => ACTIVATE_CHANNEL_OPERATION,
            SalesChannelStatus::Archived => ARCHIVE_CHANNEL_OPERATION,
        };
        let mut transaction = self.begin(actor).await?;
        if let Some(id) = reserve(&mut transaction, actor, operation, request).await? {
            return Ok(SalesChannelId::from_uuid(id));
        }
        let is_default = sqlx::query_scalar::<_, bool>(
            "SELECT is_default FROM merchant.sales_channels \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 FOR UPDATE",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| channel_not_found(sales_channel_id))?;
        if status == SalesChannelStatus::Archived {
            SalesChannel::validate_archival(is_default)?;
        }
        sqlx::query(
            "UPDATE merchant.sales_channels \
             SET status = $4::merchant.sales_channel_status, updated_at = CURRENT_TIMESTAMP \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .bind(status.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        complete(
            &mut transaction,
            actor,
            operation,
            request,
            sales_channel_id.as_uuid(),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(sales_channel_id)
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
        .ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "completed idempotency record has no administration response"
            ))
        })
}

async fn complete(
    transaction: &mut Transaction<'static, Postgres>,
    actor: MerchantActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: Uuid,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
        200,
        json!({ "data": { "id": id } }),
    )
    .await
}

async fn store_exists(
    transaction: &mut Transaction<'_, Postgres>,
    actor: MerchantActor,
    store_id: StoreId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM merchant.stores \
         WHERE merchant_account_id = $1 AND id = $2)",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)
}

async fn require_writable_store(
    transaction: &mut Transaction<'_, Postgres>,
    actor: MerchantActor,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    let writable: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM merchant.stores \
         WHERE merchant_account_id = $1 AND id = $2 AND status IN ('draft', 'active'))",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if writable {
        Ok(())
    } else {
        Err(store_not_found(store_id))
    }
}

fn store_item(row: StoreRow) -> Result<StoreAdminItem, ApplicationError> {
    let (id, code, name, region, currency, status, created_at, updated_at) = row;
    Ok(StoreAdminItem {
        id: StoreId::from_uuid(id),
        code: StoreCode::parse(code)?,
        name,
        default_region: RegionCode::parse(&region)?,
        default_currency: CurrencyCode::parse(&currency)?,
        status: StoreStatus::parse(&status).ok_or_else(corrupt_status)?,
        created_at,
        updated_at,
    })
}

fn channel_item(row: ChannelRow) -> Result<SalesChannelAdminItem, ApplicationError> {
    let (id, code, name, kind, status, is_default, created_at, updated_at) = row;
    Ok(SalesChannelAdminItem {
        id: SalesChannelId::from_uuid(id),
        code: SalesChannelCode::parse(code)?,
        name,
        kind: SalesChannelKind::parse(&kind).ok_or_else(corrupt_status)?,
        status: SalesChannelStatus::parse(&status).ok_or_else(corrupt_status)?,
        is_default,
        created_at,
        updated_at,
    })
}

fn corrupt_status() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains an unknown status"))
}

fn map_store_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error
        && database_error.constraint() == Some("stores_merchant_account_id_code_key")
    {
        return ApplicationError::Conflict {
            code: "store_code_taken",
            message: "the store code is already in use for this merchant account",
        };
    }
    database_error(error)
}

fn map_channel_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error
        && database_error.constraint()
            == Some("sales_channels_merchant_account_id_store_id_code_key")
    {
        return ApplicationError::Conflict {
            code: "sales_channel_code_taken",
            message: "the Sales Channel code is already in use for this Store",
        };
    }
    database_error(error)
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

fn channel_not_found(channel_id: SalesChannelId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "sales_channel",
        id: channel_id.as_uuid().to_string(),
    }
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chaos_application::{
        merchant::{
            ChangeSalesChannelStatusInput, ChangeStoreStatusInput, CreateSalesChannelInput,
            MerchantQueries, StoreAdministration, UpdateSalesChannelInput, UpdateStoreInput,
        },
        ports::IdempotencyRequest,
    };
    use chaos_domain::{identity::UserId, merchant::MerchantAccountId};
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    fn request(key: String, fingerprint: u8) -> IdempotencyRequest {
        IdempotencyRequest {
            key,
            request_fingerprint: [fingerprint; 32],
        }
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn administers_store_lifecycle_and_sales_channels_with_isolation() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let runtime_pool = PgPoolOptions::new()
            .max_connections(3)
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("SET ROLE chaos_runtime")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .unwrap();
        let owner_id = UserId::new();
        let support_id = UserId::new();
        let account_id = MerchantAccountId::new();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let default_channel_id = SalesChannelId::new();
        let suffix = Uuid::now_v7().simple().to_string();

        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("store-admin-{role}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Store Administration Test')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("store-admin-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query(
                "INSERT INTO merchant.merchant_account_memberships \
                 (merchant_account_id, user_id, role) \
                 VALUES ($1, $2, $3::merchant.merchant_role)",
            )
            .bind(account_id.as_uuid())
            .bind(id.as_uuid())
            .bind(role)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (id, code) in [
            (store_id, "admin-store"),
            (other_store_id, "other-admin-store"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.stores (id, merchant_account_id, code, name) \
                 VALUES ($1, $2, $3, 'Admin Store')",
            )
            .bind(id.as_uuid())
            .bind(account_id.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO merchant.store_currencies \
                 (merchant_account_id, store_id, currency) VALUES ($1, $2, 'USD')",
            )
            .bind(account_id.as_uuid())
            .bind(id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, merchant_account_id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, $3, 'web', 'Online Store', 'web', true)",
        )
        .bind(default_channel_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let queries = MerchantQueries::new(Arc::new(
            crate::repositories::PostgresMerchantReadRepository::new(runtime_pool.clone()),
        ));
        let owner = queries.authorize(owner_id, account_id).await.unwrap();
        let support = queries.authorize(support_id, account_id).await.unwrap();
        let service = StoreAdministration::new(Arc::new(
            PostgresStoreAdministrationRepository::new(runtime_pool),
        ));

        assert_eq!(
            service.get_store(owner, store_id).await.unwrap().status,
            StoreStatus::Draft
        );
        service
            .update_store(UpdateStoreInput {
                actor: owner,
                store_id,
                code: "admin-store-updated".into(),
                name: "Updated Admin Store".into(),
                default_region: "SG".into(),
                default_currency: "SGD".into(),
                idempotency: request(format!("update-store-{suffix}"), 71),
            })
            .await
            .unwrap();
        let updated = service.get_store(owner, store_id).await.unwrap();
        assert_eq!(updated.code.as_str(), "admin-store-updated");
        assert_eq!(updated.default_currency.as_str(), "SGD");
        service
            .activate_store(ChangeStoreStatusInput {
                actor: owner,
                store_id,
                idempotency: request(format!("activate-store-{suffix}"), 72),
            })
            .await
            .unwrap();
        assert_eq!(
            service.get_store(owner, store_id).await.unwrap().status,
            StoreStatus::Active
        );

        let channel_id = service
            .create_sales_channel(CreateSalesChannelInput {
                actor: owner,
                store_id,
                code: "mobile".into(),
                name: "Mobile App".into(),
                kind: "mobile".into(),
                idempotency: request(format!("create-channel-{suffix}"), 73),
            })
            .await
            .unwrap();
        let page = service
            .list_sales_channels(owner, store_id, None, 20)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 2);
        service
            .update_sales_channel(UpdateSalesChannelInput {
                actor: owner,
                store_id,
                sales_channel_id: channel_id,
                code: "mobile-app".into(),
                name: "Updated Mobile App".into(),
                kind: "mobile".into(),
                idempotency: request(format!("update-channel-{suffix}"), 74),
            })
            .await
            .unwrap();
        service
            .archive_sales_channel(ChangeSalesChannelStatusInput {
                actor: owner,
                store_id,
                sales_channel_id: channel_id,
                idempotency: request(format!("archive-channel-{suffix}"), 75),
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .get_sales_channel(owner, store_id, channel_id)
                .await
                .unwrap()
                .status,
            SalesChannelStatus::Archived
        );
        service
            .activate_sales_channel(ChangeSalesChannelStatusInput {
                actor: owner,
                store_id,
                sales_channel_id: channel_id,
                idempotency: request(format!("activate-channel-{suffix}"), 76),
            })
            .await
            .unwrap();
        let default_archive = service
            .archive_sales_channel(ChangeSalesChannelStatusInput {
                actor: owner,
                store_id,
                sales_channel_id: default_channel_id,
                idempotency: request(format!("archive-default-{suffix}"), 77),
            })
            .await;
        assert!(matches!(
            default_archive,
            Err(ApplicationError::Validation { .. })
        ));
        assert!(
            service
                .get_sales_channel(owner, other_store_id, channel_id)
                .await
                .is_err()
        );
        let forbidden = service
            .archive_store(ChangeStoreStatusInput {
                actor: support,
                store_id,
                idempotency: request(format!("support-store-{suffix}"), 78),
            })
            .await;
        assert!(matches!(forbidden, Err(ApplicationError::Forbidden)));
        service
            .archive_store(ChangeStoreStatusInput {
                actor: owner,
                store_id,
                idempotency: request(format!("archive-store-{suffix}"), 79),
            })
            .await
            .unwrap();
        assert_eq!(
            service.get_store(owner, store_id).await.unwrap().status,
            StoreStatus::Archived
        );
    }
}
