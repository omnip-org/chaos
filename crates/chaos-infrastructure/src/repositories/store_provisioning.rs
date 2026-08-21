use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{IdempotencyRequest, StoreProvisioningTransaction, StoreProvisioningUnitOfWork},
};
use chaos_domain::{
    identity::UserId,
    store::{SalesChannel, Store, StoreId, StoreMembership},
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_STORE_OPERATION: &str = "stores.create.v1";

#[derive(Clone)]
pub struct PostgresStoreProvisioningUnitOfWork {
    pool: PgPool,
}

impl PostgresStoreProvisioningUnitOfWork {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

struct PostgresStoreProvisioningTransaction {
    transaction: Transaction<'static, Postgres>,
    user_id: UserId,
}

#[async_trait]
impl StoreProvisioningUnitOfWork for PostgresStoreProvisioningUnitOfWork {
    async fn begin(
        &self,
        user_id: UserId,
    ) -> Result<Box<dyn StoreProvisioningTransaction>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(user_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(unexpected_database_error)?;
        Ok(Box::new(PostgresStoreProvisioningTransaction {
            transaction,
            user_id,
        }))
    }
}

#[async_trait]
impl StoreProvisioningTransaction for PostgresStoreProvisioningTransaction {
    async fn reserve_store_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<StoreId>, ApplicationError> {
        let Some(body) = idempotency::reserve(
            &mut self.transaction,
            &IdempotencyScope::User(self.user_id.as_uuid()),
            CREATE_STORE_OPERATION,
            request,
        )
        .await?
        else {
            return Ok(None);
        };
        let store_id = body
            .get("data")
            .and_then(|data| data.get("id"))
            .and_then(|id| id.as_str())
            .and_then(|id| Uuid::parse_str(id).ok())
            .map(StoreId::from_uuid)
            .ok_or_else(|| {
                ApplicationError::Unexpected(anyhow::anyhow!(
                    "completed idempotency record has no store response"
                ))
            })?;
        Ok(Some(store_id))
    }

    async fn insert_store(&mut self, store: &Store) -> Result<(), ApplicationError> {
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store.id().as_uuid().to_string())
            .execute(&mut *self.transaction)
            .await
            .map_err(unexpected_database_error)?;
        sqlx::query(
            "INSERT INTO commerce.stores \
             (id, code, name, default_region, default_currency, status) \
             VALUES ($1, $2, $3, $4, $5, 'inactive')",
        )
        .bind(store.id().as_uuid())
        .bind(store.code().as_str())
        .bind(store.name())
        .bind(store.default_region().as_str())
        .bind(store.default_currency().as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(map_store_write_error)?;
        Ok(())
    }

    async fn insert_owner_membership(
        &mut self,
        membership: &StoreMembership,
    ) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO commerce.store_memberships (store_id, user_id, role) \
             VALUES ($1, $2, $3::commerce.store_role)",
        )
        .bind(membership.store_id().as_uuid())
        .bind(membership.user_id().as_uuid())
        .bind(membership.role().as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        Ok(())
    }

    async fn insert_default_currency(&mut self, store: &Store) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO commerce.store_currencies \
             (store_id, currency, enabled) \
             VALUES ($1, $2, true)",
        )
        .bind(store.id().as_uuid())
        .bind(store.default_currency().as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        Ok(())
    }

    async fn insert_default_sales_channel(
        &mut self,
        channel: &SalesChannel,
    ) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO commerce.sales_channels \
             (id, store_id, code, name, kind, status, is_default) \
             VALUES ($1, $2, $3, $4, \
                     $5::commerce.sales_channel_kind, \
                     $6::commerce.sales_channel_status, $7)",
        )
        .bind(channel.id().as_uuid())
        .bind(channel.store_id().as_uuid())
        .bind(channel.code().as_str())
        .bind(channel.name())
        .bind(channel.kind().as_str())
        .bind(channel.status().as_str())
        .bind(channel.is_default())
        .execute(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        Ok(())
    }

    async fn complete_store_creation(
        &mut self,
        request: &IdempotencyRequest,
        store_id: StoreId,
    ) -> Result<(), ApplicationError> {
        idempotency::complete(
            &mut self.transaction,
            &IdempotencyScope::User(self.user_id.as_uuid()),
            CREATE_STORE_OPERATION,
            request,
            201,
            json!({ "data": { "id": store_id.as_uuid() } }),
        )
        .await
    }

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError> {
        self.transaction
            .commit()
            .await
            .map_err(unexpected_database_error)
    }
}

fn map_store_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error
        && database_error.constraint() == Some("stores_code_key")
    {
        return ApplicationError::Conflict {
            code: "store_code_taken",
            message: "the store code is already in use",
        };
    }
    unexpected_database_error(error)
}

fn unexpected_database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chaos_application::store::{CreateStore, CreateStoreInput};
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn provisions_a_store_with_owner_membership_currency_and_idempotency() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let runtime_pool = PgPoolOptions::new()
            .max_connections(2)
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
        let owner_user_id = UserId::new();
        let unique_suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();
        let idempotency_key = format!("store-{unique_suffix}");

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_user_id.as_uuid())
            .bind(format!("store-owner-{unique_suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();

        let service = CreateStore::new(Arc::new(PostgresStoreProvisioningUnitOfWork::new(
            runtime_pool,
        )));
        let make_input = |fingerprint| CreateStoreInput {
            user_id: owner_user_id,
            code: format!("primary-{unique_suffix}"),
            name: "Primary Store".into(),
            default_region: None,
            default_currency: None,
            idempotency: IdempotencyRequest {
                key: idempotency_key.clone(),
                request_fingerprint: fingerprint,
            },
        };

        let output = service.execute(make_input([21; 32])).await.unwrap();
        let replay = service.execute(make_input([21; 32])).await.unwrap();
        assert_eq!(replay.store_id, output.store_id);

        let mismatch = service.execute(make_input([22; 32])).await;
        assert!(matches!(
            mismatch,
            Err(ApplicationError::Conflict {
                code: "idempotency_key_reused",
                ..
            })
        ));

        let stored: (String, String, String, bool, String, String, bool) = sqlx::query_as(
            "SELECT store.status::text, store.default_region::text, \
                    currency.currency::text, currency.enabled, \
                    channel.code::text, channel.kind::text, channel.is_default \
             FROM commerce.stores AS store \
             INNER JOIN commerce.store_currencies AS currency \
                 ON currency.store_id = store.id \
             INNER JOIN commerce.sales_channels AS channel \
                 ON channel.store_id = store.id \
             WHERE store.id = $1",
        )
        .bind(output.store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            stored,
            (
                "inactive".into(),
                "US".into(),
                "USD".into(),
                true,
                "web".into(),
                "web".into(),
                true,
            )
        );

        let membership_role: String = sqlx::query_scalar(
            "SELECT role::text FROM commerce.store_memberships \
             WHERE store_id = $1 AND user_id = $2",
        )
        .bind(output.store_id.as_uuid())
        .bind(owner_user_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(membership_role, "owner");

        sqlx::query("DELETE FROM commerce.stores WHERE id = $1")
            .bind(output.store_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM integration.idempotency_keys \
             WHERE scope = 'user' AND scope_id = $1",
        )
        .bind(owner_user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(owner_user_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
