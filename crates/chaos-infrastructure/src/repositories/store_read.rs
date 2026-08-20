use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{StoreListItem, StoreReadRepository},
};
use chaos_domain::{
    CurrencyCode, RegionCode,
    identity::UserId,
    store::{StoreCode, StoreId, StoreRole, StoreStatus},
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresStoreReadRepository {
    pool: PgPool,
}

impl PostgresStoreReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl StoreReadRepository for PostgresStoreReadRepository {
    async fn membership_role(
        &self,
        user_id: UserId,
        store_id: StoreId,
    ) -> Result<Option<StoreRole>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        set_user_context(&mut transaction, user_id).await?;
        let role = sqlx::query_scalar::<_, String>(
            "SELECT role::text \
             FROM commerce.store_memberships \
             WHERE store_id = $1 AND user_id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(user_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(unexpected_database_error)?;
        transaction
            .commit()
            .await
            .map_err(unexpected_database_error)?;

        role.map(|value| {
            StoreRole::parse(&value).ok_or_else(|| corrupt_database_enum("store role", &value))
        })
        .transpose()
    }

    async fn list_stores(
        &self,
        user_id: UserId,
        after: Option<StoreId>,
        limit: u16,
    ) -> Result<Vec<StoreListItem>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        set_user_context(&mut transaction, user_id).await?;
        let rows = sqlx::query_as::<_, (Uuid, String, String, String, String, String, String)>(
            "SELECT store.id, store.code::text, store.name, store.default_region::text, \
                    store.default_currency::text, store.status::text, membership.role::text \
             FROM commerce.store_memberships AS membership \
             INNER JOIN commerce.stores AS store ON store.id = membership.store_id \
             WHERE membership.user_id = $1 \
               AND ($2::uuid IS NULL OR store.id > $2) \
             ORDER BY store.id ASC \
             LIMIT $3",
        )
        .bind(user_id.as_uuid())
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
            .map(|(id, code, name, region, currency, status, role)| {
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
                    role: StoreRole::parse(&role)
                        .ok_or_else(|| corrupt_database_enum("store role", &role))?,
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

fn corrupt_database_value(error: chaos_domain::DomainError) -> ApplicationError {
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
    async fn lists_only_authorized_stores_with_keyset_pagination() {
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
        let first_store_id = StoreId::new();
        let second_store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let unique_suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        for (id, label) in [(user_id, "reader"), (other_user_id, "other")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("directory-{label}-{unique_suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        for (id, code, name) in [
            (first_store_id, format!("first-{unique_suffix}"), "First"),
            (second_store_id, format!("second-{unique_suffix}"), "Second"),
            (other_store_id, format!("other-{unique_suffix}"), "Other"),
        ] {
            sqlx::query("INSERT INTO commerce.stores (id, code, name) VALUES ($1, $2, $3)")
                .bind(id.as_uuid())
                .bind(code)
                .bind(name)
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        for (store_id, member_id) in [
            (first_store_id, user_id),
            (second_store_id, user_id),
            (other_store_id, other_user_id),
        ] {
            sqlx::query(
                "INSERT INTO commerce.store_memberships \
                 (store_id, user_id, role) VALUES ($1, $2, 'owner')",
            )
            .bind(store_id.as_uuid())
            .bind(member_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }

        let repository = PostgresStoreReadRepository::new(runtime_pool);
        let first_page = repository.list_stores(user_id, None, 1).await.unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].id, first_store_id);
        let second_page = repository
            .list_stores(user_id, Some(first_page[0].id), 2)
            .await
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].id, second_store_id);

        assert_eq!(
            repository
                .membership_role(user_id, first_store_id)
                .await
                .unwrap(),
            Some(StoreRole::Owner)
        );
        assert_eq!(
            repository
                .membership_role(user_id, other_store_id)
                .await
                .unwrap(),
            None
        );

        sqlx::query("DELETE FROM commerce.stores WHERE id = ANY($1)")
            .bind(vec![
                first_store_id.as_uuid(),
                second_store_id.as_uuid(),
                other_store_id.as_uuid(),
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
