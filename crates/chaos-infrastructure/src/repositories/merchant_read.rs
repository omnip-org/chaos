use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{MerchantAccountListItem, MerchantReadRepository, StoreListItem},
};
use chaos_domain::{
    CurrencyCode, DomainError, RegionCode,
    identity::UserId,
    merchant::{
        MerchantAccountId, MerchantAccountSlug, MerchantAccountStatus, MerchantRole, StoreCode,
        StoreId, StoreStatus,
    },
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresMerchantReadRepository {
    pool: PgPool,
}

impl PostgresMerchantReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MerchantReadRepository for PostgresMerchantReadRepository {
    async fn list_merchant_accounts(
        &self,
        user_id: UserId,
        after: Option<MerchantAccountId>,
        limit: u16,
    ) -> Result<Vec<MerchantAccountListItem>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        set_user_context(&mut transaction, user_id).await?;
        let rows = sqlx::query_as::<_, (Uuid, String, String, String, String)>(
            "SELECT account.id, account.slug::text, account.display_name, \
                    account.status::text, membership.role::text \
             FROM merchant.merchant_account_memberships AS membership \
             INNER JOIN merchant.merchant_accounts AS account \
                ON account.id = membership.merchant_account_id \
             WHERE membership.user_id = $1 \
               AND ($2::uuid IS NULL OR account.id > $2) \
             ORDER BY account.id ASC \
             LIMIT $3",
        )
        .bind(user_id.as_uuid())
        .bind(after.map(MerchantAccountId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(unexpected_database_error)?;
        transaction
            .commit()
            .await
            .map_err(unexpected_database_error)?;

        rows.into_iter()
            .map(|(id, slug, display_name, status, role)| {
                Ok(MerchantAccountListItem {
                    id: MerchantAccountId::from_uuid(id),
                    slug: MerchantAccountSlug::parse(slug).map_err(corrupt_database_value)?,
                    display_name,
                    status: MerchantAccountStatus::parse(&status)
                        .ok_or_else(|| corrupt_database_enum("merchant account status", &status))?,
                    role: MerchantRole::parse(&role)
                        .ok_or_else(|| corrupt_database_enum("merchant role", &role))?,
                })
            })
            .collect()
    }

    async fn membership_role(
        &self,
        user_id: UserId,
        merchant_account_id: MerchantAccountId,
    ) -> Result<Option<MerchantRole>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        set_user_context(&mut transaction, user_id).await?;
        set_merchant_context(&mut transaction, merchant_account_id).await?;
        let role = sqlx::query_scalar::<_, String>(
            "SELECT role::text \
             FROM merchant.merchant_account_memberships \
             WHERE merchant_account_id = $1 AND user_id = $2",
        )
        .bind(merchant_account_id.as_uuid())
        .bind(user_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(unexpected_database_error)?;
        transaction
            .commit()
            .await
            .map_err(unexpected_database_error)?;

        role.map(|value| {
            MerchantRole::parse(&value)
                .ok_or_else(|| corrupt_database_enum("merchant role", &value))
        })
        .transpose()
    }

    async fn list_stores(
        &self,
        user_id: UserId,
        merchant_account_id: MerchantAccountId,
        after: Option<StoreId>,
        limit: u16,
    ) -> Result<Vec<StoreListItem>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        set_user_context(&mut transaction, user_id).await?;
        set_merchant_context(&mut transaction, merchant_account_id).await?;
        let rows = sqlx::query_as::<_, (Uuid, String, String, String, String, String)>(
            "SELECT id, code::text, name, default_region::text, \
                    default_currency::text, status::text \
             FROM merchant.stores \
             WHERE merchant_account_id = $1 \
               AND ($2::uuid IS NULL OR id > $2) \
             ORDER BY id ASC \
             LIMIT $3",
        )
        .bind(merchant_account_id.as_uuid())
        .bind(after.map(StoreId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(unexpected_database_error)?;
        transaction
            .commit()
            .await
            .map_err(unexpected_database_error)?;

        rows.into_iter()
            .map(|(id, code, name, region, currency, status)| {
                Ok(StoreListItem {
                    id: StoreId::from_uuid(id),
                    code: StoreCode::parse(code).map_err(corrupt_database_value)?,
                    name,
                    default_region: RegionCode::parse(region.trim_end())
                        .map_err(corrupt_database_value)?,
                    default_currency: CurrencyCode::parse(currency.trim_end())
                        .map_err(corrupt_database_value)?,
                    status: StoreStatus::parse(&status)
                        .ok_or_else(|| corrupt_database_enum("store status", &status))?,
                })
            })
            .collect()
    }
}

async fn set_user_context(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: UserId,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT set_config('app.user_id', $1, true)")
        .bind(user_id.as_uuid().to_string())
        .execute(&mut **transaction)
        .await
        .map_err(unexpected_database_error)?;
    Ok(())
}

async fn set_merchant_context(
    transaction: &mut Transaction<'_, Postgres>,
    merchant_account_id: MerchantAccountId,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
        .bind(merchant_account_id.as_uuid().to_string())
        .execute(&mut **transaction)
        .await
        .map_err(unexpected_database_error)?;
    Ok(())
}

fn corrupt_database_value(error: DomainError) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database invariant violation: {error}"))
}

fn corrupt_database_enum(name: &str, value: &str) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains unknown {name}: {value}"))
}

fn unexpected_database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn lists_only_authorized_accounts_and_stores_with_keyset_pagination() {
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
        let user_id = UserId::new();
        let other_user_id = UserId::new();
        let first_account_id = MerchantAccountId::new();
        let second_account_id = MerchantAccountId::new();
        let other_account_id = MerchantAccountId::new();
        let first_store_id = StoreId::new();
        let second_store_id = StoreId::new();
        let unique_suffix = Uuid::now_v7().simple().to_string();

        for (id, label) in [(user_id, "reader"), (other_user_id, "other")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("directory-{label}-{unique_suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        for (id, slug, name) in [
            (first_account_id, format!("first-{unique_suffix}"), "First"),
            (
                second_account_id,
                format!("second-{unique_suffix}"),
                "Second",
            ),
            (other_account_id, format!("other-{unique_suffix}"), "Other"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
                 VALUES ($1, $2, $3)",
            )
            .bind(id.as_uuid())
            .bind(slug)
            .bind(name)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (account_id, member_id) in [
            (first_account_id, user_id),
            (second_account_id, user_id),
            (other_account_id, other_user_id),
        ] {
            sqlx::query(
                "INSERT INTO merchant.merchant_account_memberships \
                 (merchant_account_id, user_id, role) VALUES ($1, $2, 'owner')",
            )
            .bind(account_id.as_uuid())
            .bind(member_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (id, code) in [(first_store_id, "first"), (second_store_id, "second")] {
            sqlx::query(
                "INSERT INTO merchant.stores \
                 (id, merchant_account_id, code, name) VALUES ($1, $2, $3, $4)",
            )
            .bind(id.as_uuid())
            .bind(first_account_id.as_uuid())
            .bind(code)
            .bind(format!("{code} Store"))
            .execute(&owner_pool)
            .await
            .unwrap();
        }

        let repository = PostgresMerchantReadRepository::new(runtime_pool);
        let first_page = repository
            .list_merchant_accounts(user_id, None, 1)
            .await
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].id, first_account_id);
        let second_page = repository
            .list_merchant_accounts(user_id, Some(first_page[0].id), 2)
            .await
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].id, second_account_id);

        assert_eq!(
            repository
                .membership_role(user_id, first_account_id)
                .await
                .unwrap(),
            Some(MerchantRole::Owner)
        );
        assert_eq!(
            repository
                .membership_role(user_id, other_account_id)
                .await
                .unwrap(),
            None
        );

        let stores = repository
            .list_stores(user_id, first_account_id, Some(first_store_id), 2)
            .await
            .unwrap();
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].id, second_store_id);
        assert_eq!(stores[0].default_region, RegionCode::US);
        assert_eq!(stores[0].default_currency, CurrencyCode::USD);

        sqlx::query("DELETE FROM merchant.stores WHERE merchant_account_id = ANY($1)")
            .bind(vec![
                first_account_id.as_uuid(),
                second_account_id.as_uuid(),
                other_account_id.as_uuid(),
            ])
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM merchant.merchant_accounts WHERE id = ANY($1)")
            .bind(vec![
                first_account_id.as_uuid(),
                second_account_id.as_uuid(),
                other_account_id.as_uuid(),
            ])
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = ANY($1)")
            .bind(vec![user_id.as_uuid(), other_user_id.as_uuid()])
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
