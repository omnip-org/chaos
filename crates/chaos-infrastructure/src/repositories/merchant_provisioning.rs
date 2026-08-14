use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{IdempotencyRequest, MerchantProvisioningTransaction, MerchantProvisioningUnitOfWork},
};
use chaos_domain::{
    identity::UserId,
    merchant::{MerchantAccount, MerchantAccountId, MerchantAccountMembership},
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const CREATE_ACCOUNT_OPERATION: &str = "merchant_accounts.create.v1";

#[derive(Clone)]
pub struct PostgresMerchantProvisioningUnitOfWork {
    pool: PgPool,
}

impl PostgresMerchantProvisioningUnitOfWork {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

struct PostgresMerchantProvisioningTransaction {
    transaction: Transaction<'static, Postgres>,
}

#[async_trait]
impl MerchantProvisioningUnitOfWork for PostgresMerchantProvisioningUnitOfWork {
    async fn begin(
        &self,
        owner_user_id: UserId,
        merchant_account_id: MerchantAccountId,
    ) -> Result<Box<dyn MerchantProvisioningTransaction>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(owner_user_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(unexpected_database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(merchant_account_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(unexpected_database_error)?;
        Ok(Box::new(PostgresMerchantProvisioningTransaction {
            transaction,
        }))
    }
}

#[async_trait]
impl MerchantProvisioningTransaction for PostgresMerchantProvisioningTransaction {
    async fn reserve_account_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<MerchantAccountId>, ApplicationError> {
        let inserted = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO integration.idempotency_records \
             (id, scope, scope_id, operation, idempotency_key, request_fingerprint) \
             VALUES (uuidv7(), 'user', \
                nullif(current_setting('app.user_id', true), '')::uuid, $1, $2, $3) \
             ON CONFLICT (scope, scope_id, operation, idempotency_key) DO NOTHING \
             RETURNING id",
        )
        .bind(CREATE_ACCOUNT_OPERATION)
        .bind(&request.key)
        .bind(request.request_fingerprint.as_slice())
        .fetch_optional(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;

        if inserted.is_some() {
            return Ok(None);
        }

        let record = sqlx::query_as::<_, (Vec<u8>, Option<serde_json::Value>)>(
            "SELECT request_fingerprint, response_body \
             FROM integration.idempotency_records \
             WHERE scope = 'user' \
               AND scope_id = nullif(current_setting('app.user_id', true), '')::uuid \
               AND operation = $1 \
               AND idempotency_key = $2 \
             FOR UPDATE",
        )
        .bind(CREATE_ACCOUNT_OPERATION)
        .bind(&request.key)
        .fetch_one(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;

        if record.0.as_slice() != request.request_fingerprint {
            return Err(ApplicationError::Conflict {
                code: "idempotency_key_reused",
                message: "the idempotency key was already used with a different request",
            });
        }

        let merchant_account_id = record
            .1
            .and_then(|body| body.get("data")?.get("id")?.as_str().map(str::to_owned))
            .and_then(|id| Uuid::parse_str(&id).ok())
            .map(MerchantAccountId::from_uuid)
            .ok_or_else(|| {
                ApplicationError::Unexpected(anyhow::anyhow!(
                    "completed idempotency record has no merchant account response"
                ))
            })?;
        Ok(Some(merchant_account_id))
    }

    async fn insert_merchant_account(
        &mut self,
        account: &MerchantAccount,
    ) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts \
             (id, slug, display_name, status) \
             VALUES ($1, $2, $3, 'active')",
        )
        .bind(account.id().as_uuid())
        .bind(account.slug().as_str())
        .bind(account.display_name())
        .execute(&mut *self.transaction)
        .await
        .map_err(map_merchant_write_error)?;
        Ok(())
    }

    async fn insert_membership(
        &mut self,
        membership: &MerchantAccountMembership,
    ) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO merchant.merchant_account_memberships \
             (merchant_account_id, user_id, role) \
             VALUES ($1, $2, $3::merchant.merchant_role)",
        )
        .bind(membership.merchant_account_id().as_uuid())
        .bind(membership.user_id().as_uuid())
        .bind(membership.role().as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        Ok(())
    }

    async fn complete_account_creation(
        &mut self,
        request: &IdempotencyRequest,
        merchant_account_id: MerchantAccountId,
    ) -> Result<(), ApplicationError> {
        let result = sqlx::query(
            "UPDATE integration.idempotency_records \
             SET response_status = 201, \
                 response_body = $3, \
                 completed_at = CURRENT_TIMESTAMP, \
                 updated_at = CURRENT_TIMESTAMP \
             WHERE scope = 'user' \
               AND scope_id = nullif(current_setting('app.user_id', true), '')::uuid \
               AND operation = $1 \
               AND idempotency_key = $2",
        )
        .bind(CREATE_ACCOUNT_OPERATION)
        .bind(&request.key)
        .bind(json!({ "data": { "id": merchant_account_id.as_uuid() } }))
        .execute(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        if result.rows_affected() != 1 {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "idempotency record disappeared before completion"
            )));
        }
        Ok(())
    }

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError> {
        self.transaction
            .commit()
            .await
            .map_err(unexpected_database_error)
    }
}

fn map_merchant_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error
        && database_error.constraint() == Some("merchant_accounts_slug_key")
    {
        return ApplicationError::Conflict {
            code: "merchant_account_slug_taken",
            message: "the merchant account slug is already in use",
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

    use chaos_application::merchant::{CreateMerchantAccount, CreateMerchantAccountInput};
    use chaos_domain::identity::UserId;
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn provisions_account_and_owner_membership_in_one_transaction() {
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
        let unique_suffix = Uuid::now_v7().simple().to_string();
        let slug = format!("provision-{unique_suffix}");

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_user_id.as_uuid())
            .bind(format!("owner-{unique_suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();

        let service = CreateMerchantAccount::new(Arc::new(
            PostgresMerchantProvisioningUnitOfWork::new(runtime_pool.clone()),
        ));
        let idempotency_key = format!("provision-{unique_suffix}");
        let output = service
            .execute(CreateMerchantAccountInput {
                owner_user_id,
                slug: slug.clone(),
                display_name: "Provisioning Test".into(),
                idempotency: IdempotencyRequest {
                    key: idempotency_key.clone(),
                    request_fingerprint: [11; 32],
                },
            })
            .await
            .unwrap();
        let replay = service
            .execute(CreateMerchantAccountInput {
                owner_user_id,
                slug: slug.clone(),
                display_name: "Provisioning Test".into(),
                idempotency: IdempotencyRequest {
                    key: idempotency_key.clone(),
                    request_fingerprint: [11; 32],
                },
            })
            .await
            .unwrap();
        assert_eq!(replay.merchant_account_id, output.merchant_account_id);

        let mismatch = service
            .execute(CreateMerchantAccountInput {
                owner_user_id,
                slug,
                display_name: "Provisioning Test".into(),
                idempotency: IdempotencyRequest {
                    key: idempotency_key.clone(),
                    request_fingerprint: [12; 32],
                },
            })
            .await
            .unwrap_err();
        assert!(matches!(
            mismatch,
            ApplicationError::Conflict {
                code: "idempotency_key_reused",
                ..
            }
        ));

        let role: String = sqlx::query_scalar(
            "SELECT role::text FROM merchant.merchant_account_memberships \
             WHERE merchant_account_id = $1 AND user_id = $2",
        )
        .bind(output.merchant_account_id.as_uuid())
        .bind(owner_user_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(role, "owner");

        let account_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM merchant.merchant_account_memberships WHERE user_id = $1",
        )
        .bind(owner_user_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(account_count, 1);

        let mut isolated_transaction = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(UserId::new().as_uuid().to_string())
            .execute(&mut *isolated_transaction)
            .await
            .unwrap();
        let visible_idempotency_records: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM integration.idempotency_records \
             WHERE operation = $1 AND idempotency_key = $2",
        )
        .bind(CREATE_ACCOUNT_OPERATION)
        .bind(&idempotency_key)
        .fetch_one(&mut *isolated_transaction)
        .await
        .unwrap();
        assert_eq!(visible_idempotency_records, 0);
        isolated_transaction.rollback().await.unwrap();

        sqlx::query("DELETE FROM merchant.merchant_accounts WHERE id = $1")
            .bind(output.merchant_account_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM integration.idempotency_records \
             WHERE scope = 'user' AND scope_id = $1 AND idempotency_key = $2",
        )
        .bind(owner_user_id.as_uuid())
        .bind(idempotency_key)
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
