use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{IdempotencyRequest, StoreProvisioningTransaction, StoreProvisioningUnitOfWork},
};
use chaos_domain::{
    identity::UserId,
    merchant::{MerchantAccountId, Store, StoreId},
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
    merchant_account_id: MerchantAccountId,
}

#[async_trait]
impl StoreProvisioningUnitOfWork for PostgresStoreProvisioningUnitOfWork {
    async fn begin(
        &self,
        user_id: UserId,
        merchant_account_id: MerchantAccountId,
    ) -> Result<Box<dyn StoreProvisioningTransaction>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(user_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(unexpected_database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(merchant_account_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(unexpected_database_error)?;
        Ok(Box::new(PostgresStoreProvisioningTransaction {
            transaction,
            merchant_account_id,
        }))
    }
}

#[async_trait]
impl StoreProvisioningTransaction for PostgresStoreProvisioningTransaction {
    async fn can_create_store(&mut self, user_id: UserId) -> Result<bool, ApplicationError> {
        sqlx::query_scalar(
            "SELECT EXISTS (\
                SELECT 1 \
                FROM merchant.merchant_account_memberships AS membership \
                INNER JOIN merchant.merchant_accounts AS account \
                    ON account.id = membership.merchant_account_id \
                WHERE membership.merchant_account_id = \
                    nullif(current_setting('app.merchant_account_id', true), '')::uuid \
                  AND membership.user_id = $1 \
                  AND membership.role IN ('owner', 'administrator') \
                  AND account.status = 'active'\
             )",
        )
        .bind(user_id.as_uuid())
        .fetch_one(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)
    }

    async fn reserve_store_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<StoreId>, ApplicationError> {
        let Some(body) = idempotency::reserve(
            &mut self.transaction,
            &IdempotencyScope::MerchantAccount(self.merchant_account_id.as_uuid()),
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
        sqlx::query(
            "INSERT INTO merchant.stores \
             (id, merchant_account_id, code, name, default_currency, status) \
             VALUES ($1, $2, $3, $4, $5, 'draft')",
        )
        .bind(store.id().as_uuid())
        .bind(store.merchant_account_id().as_uuid())
        .bind(store.code().as_str())
        .bind(store.name())
        .bind(store.default_currency().as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(map_store_write_error)?;
        Ok(())
    }

    async fn insert_default_currency(&mut self, store: &Store) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO merchant.store_currencies \
             (merchant_account_id, store_id, currency, enabled) \
             VALUES ($1, $2, $3, true)",
        )
        .bind(store.merchant_account_id().as_uuid())
        .bind(store.id().as_uuid())
        .bind(store.default_currency().as_str())
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
            &IdempotencyScope::MerchantAccount(self.merchant_account_id.as_uuid()),
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
        && database_error.constraint() == Some("stores_merchant_account_id_code_key")
    {
        return ApplicationError::Conflict {
            code: "store_code_taken",
            message: "the store code is already in use for this merchant account",
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

    use chaos_application::merchant::{CreateStore, CreateStoreInput};
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn provisions_a_store_with_authorization_currency_and_idempotency() {
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
        let support_user_id = UserId::new();
        let merchant_account_id = MerchantAccountId::new();
        let unique_suffix = Uuid::now_v7().simple().to_string();
        let idempotency_key = format!("store-{unique_suffix}");

        for (user_id, label) in [(owner_user_id, "owner"), (support_user_id, "support")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(user_id.as_uuid())
                .bind(format!("store-{label}-{unique_suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Store Provisioning Test')",
        )
        .bind(merchant_account_id.as_uuid())
        .bind(format!("store-test-{unique_suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (user_id, role) in [(owner_user_id, "owner"), (support_user_id, "support")] {
            sqlx::query(
                "INSERT INTO merchant.merchant_account_memberships \
                 (merchant_account_id, user_id, role) \
                 VALUES ($1, $2, $3::merchant.merchant_role)",
            )
            .bind(merchant_account_id.as_uuid())
            .bind(user_id.as_uuid())
            .bind(role)
            .execute(&owner_pool)
            .await
            .unwrap();
        }

        let service = CreateStore::new(Arc::new(PostgresStoreProvisioningUnitOfWork::new(
            runtime_pool,
        )));
        let make_input = |user_id, fingerprint| CreateStoreInput {
            user_id,
            merchant_account_id,
            code: "primary".into(),
            name: "Primary Store".into(),
            default_currency: "SGD".into(),
            idempotency: IdempotencyRequest {
                key: idempotency_key.clone(),
                request_fingerprint: fingerprint,
            },
        };

        let forbidden = service.execute(make_input(support_user_id, [20; 32])).await;
        assert!(matches!(forbidden, Err(ApplicationError::Forbidden)));

        let output = service
            .execute(make_input(owner_user_id, [21; 32]))
            .await
            .unwrap();
        let replay = service
            .execute(make_input(owner_user_id, [21; 32]))
            .await
            .unwrap();
        assert_eq!(replay.store_id, output.store_id);

        let mismatch = service.execute(make_input(owner_user_id, [22; 32])).await;
        assert!(matches!(
            mismatch,
            Err(ApplicationError::Conflict {
                code: "idempotency_key_reused",
                ..
            })
        ));

        let stored: (String, String, bool) = sqlx::query_as(
            "SELECT store.status::text, currency.currency::text, currency.enabled \
             FROM merchant.stores AS store \
             INNER JOIN merchant.store_currencies AS currency \
                 ON currency.merchant_account_id = store.merchant_account_id \
                AND currency.store_id = store.id \
             WHERE store.id = $1",
        )
        .bind(output.store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(stored, ("draft".into(), "SGD".into(), true));

        let store_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM merchant.stores WHERE merchant_account_id = $1",
        )
        .bind(merchant_account_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(store_count, 1);

        sqlx::query("DELETE FROM merchant.stores WHERE merchant_account_id = $1")
            .bind(merchant_account_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM merchant.merchant_accounts WHERE id = $1")
            .bind(merchant_account_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM integration.idempotency_records \
             WHERE scope = 'merchant_account' AND scope_id = $1",
        )
        .bind(merchant_account_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = ANY($1)")
            .bind(vec![owner_user_id.as_uuid(), support_user_id.as_uuid()])
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
