use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{MerchantProvisioningTransaction, MerchantProvisioningUnitOfWork},
};
use chaos_domain::merchant::{MerchantAccount, MerchantAccountMembership};
use sqlx::{PgPool, Postgres, Transaction};

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
    async fn begin(&self) -> Result<Box<dyn MerchantProvisioningTransaction>, ApplicationError> {
        let transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        Ok(Box::new(PostgresMerchantProvisioningTransaction {
            transaction,
        }))
    }
}

#[async_trait]
impl MerchantProvisioningTransaction for PostgresMerchantProvisioningTransaction {
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
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let owner_user_id = UserId::new();
        let unique_suffix = Uuid::now_v7().simple().to_string();
        let slug = format!("provision-{unique_suffix}");

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_user_id.as_uuid())
            .bind(format!("owner-{unique_suffix}@example.com"))
            .execute(&pool)
            .await
            .unwrap();

        let service = CreateMerchantAccount::new(Arc::new(
            PostgresMerchantProvisioningUnitOfWork::new(pool.clone()),
        ));
        let output = service
            .execute(CreateMerchantAccountInput {
                owner_user_id,
                slug,
                display_name: "Provisioning Test".into(),
            })
            .await
            .unwrap();

        let role: String = sqlx::query_scalar(
            "SELECT role::text FROM merchant.merchant_account_memberships \
             WHERE merchant_account_id = $1 AND user_id = $2",
        )
        .bind(output.merchant_account_id.as_uuid())
        .bind(owner_user_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(role, "owner");

        sqlx::query("DELETE FROM merchant.merchant_accounts WHERE id = $1")
            .bind(output.merchant_account_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(owner_user_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
    }
}
