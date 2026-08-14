use anyhow::Context;
use chaos_domain::merchant::MerchantAccountId;
use sqlx::{PgPool, Postgres, Transaction};

#[cfg(test)]
use sqlx::PgConnection;

pub struct MerchantAccountTransaction<'a> {
    inner: Transaction<'a, Postgres>,
    merchant_account_id: MerchantAccountId,
}

impl<'a> MerchantAccountTransaction<'a> {
    pub(crate) async fn begin(
        pool: &'a PgPool,
        merchant_account_id: MerchantAccountId,
    ) -> anyhow::Result<Self> {
        let mut inner = pool
            .begin()
            .await
            .context("failed to begin merchant account transaction")?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(merchant_account_id.as_uuid().to_string())
            .execute(&mut *inner)
            .await
            .context("failed to establish PostgreSQL merchant account context")?;

        Ok(Self {
            inner,
            merchant_account_id,
        })
    }

    pub const fn merchant_account_id(&self) -> MerchantAccountId {
        self.merchant_account_id
    }

    #[cfg(test)]
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        &mut self.inner
    }

    pub async fn commit(self) -> anyhow::Result<()> {
        self.inner
            .commit()
            .await
            .context("failed to commit merchant account transaction")
    }

    pub async fn rollback(self) -> anyhow::Result<()> {
        self.inner
            .rollback()
            .await
            .context("failed to roll back merchant account transaction")
    }
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn rls_hides_other_merchant_accounts_rows() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let public_business_tables: i64 = sqlx::query_scalar(
            "SELECT count(*) \
             FROM pg_catalog.pg_tables \
             WHERE schemaname = 'public' AND tablename <> '_sqlx_migrations'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(public_business_tables, 0);

        let account_a = MerchantAccountId::new();
        let account_b = MerchantAccountId::new();
        let store_a = Uuid::now_v7();
        let store_b = Uuid::now_v7();

        for (account_id, slug) in [
            (account_a, "rls-test-account-a"),
            (account_b, "rls-test-account-b"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
                 VALUES ($1, $2, $3)",
            )
            .bind(account_id.as_uuid())
            .bind(format!("{slug}-{}", Uuid::now_v7().simple()))
            .bind(slug)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (store_id, account_id, code) in [
            (store_a, account_a, "store-a"),
            (store_b, account_b, "store-b"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.stores \
                 (id, merchant_account_id, code, name, default_currency) \
                 VALUES ($1, $2, $3, $4, 'USD')",
            )
            .bind(store_id)
            .bind(account_id.as_uuid())
            .bind(code)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
        }

        let mut transaction = MerchantAccountTransaction::begin(&pool, account_a)
            .await
            .unwrap();
        sqlx::query("SET LOCAL ROLE chaos_runtime")
            .execute(transaction.connection())
            .await
            .unwrap();
        let visible_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM merchant.stores ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        transaction.rollback().await.unwrap();

        assert_eq!(visible_ids, vec![store_a]);

        sqlx::query("DELETE FROM merchant.stores WHERE merchant_account_id = ANY($1)")
            .bind(vec![account_a.as_uuid(), account_b.as_uuid()])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM merchant.merchant_accounts WHERE id = ANY($1)")
            .bind(vec![account_a.as_uuid(), account_b.as_uuid()])
            .execute(&pool)
            .await
            .unwrap();
    }
}
