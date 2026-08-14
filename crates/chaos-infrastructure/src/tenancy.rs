use anyhow::Context;
use chaos_domain::tenancy::TenantId;
use sqlx::{PgPool, Postgres, Transaction};

#[cfg(test)]
use sqlx::PgConnection;

pub struct TenantTransaction<'a> {
    inner: Transaction<'a, Postgres>,
    tenant_id: TenantId,
}

impl<'a> TenantTransaction<'a> {
    pub(crate) async fn begin(pool: &'a PgPool, tenant_id: TenantId) -> anyhow::Result<Self> {
        let mut inner = pool
            .begin()
            .await
            .context("failed to begin tenant transaction")?;
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(tenant_id.as_uuid().to_string())
            .execute(&mut *inner)
            .await
            .context("failed to establish PostgreSQL tenant context")?;

        Ok(Self { inner, tenant_id })
    }

    pub const fn tenant_id(&self) -> TenantId {
        self.tenant_id
    }

    #[cfg(test)]
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        &mut self.inner
    }

    pub async fn commit(self) -> anyhow::Result<()> {
        self.inner
            .commit()
            .await
            .context("failed to commit tenant transaction")
    }

    pub async fn rollback(self) -> anyhow::Result<()> {
        self.inner
            .rollback()
            .await
            .context("failed to roll back tenant transaction")
    }
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn rls_hides_other_tenants_rows() {
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

        let tenant_a = TenantId::new();
        let tenant_b = TenantId::new();
        let store_a = Uuid::now_v7();
        let store_b = Uuid::now_v7();

        for (tenant_id, slug) in [(tenant_a, "rls-test-a"), (tenant_b, "rls-test-b")] {
            sqlx::query("INSERT INTO tenancy.tenants (id, slug, display_name) VALUES ($1, $2, $3)")
                .bind(tenant_id.as_uuid())
                .bind(format!("{slug}-{}", Uuid::now_v7().simple()))
                .bind(slug)
                .execute(&pool)
                .await
                .unwrap();
        }
        for (store_id, tenant_id, code) in [
            (store_a, tenant_a, "store-a"),
            (store_b, tenant_b, "store-b"),
        ] {
            sqlx::query(
                "INSERT INTO tenancy.stores (id, tenant_id, code, name, default_currency) \
                 VALUES ($1, $2, $3, $4, 'USD')",
            )
            .bind(store_id)
            .bind(tenant_id.as_uuid())
            .bind(code)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
        }

        let mut transaction = TenantTransaction::begin(&pool, tenant_a).await.unwrap();
        sqlx::query("SET LOCAL ROLE chaos_runtime")
            .execute(transaction.connection())
            .await
            .unwrap();
        let visible_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM tenancy.stores ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        transaction.rollback().await.unwrap();

        assert_eq!(visible_ids, vec![store_a]);

        sqlx::query("DELETE FROM tenancy.stores WHERE tenant_id = ANY($1)")
            .bind(vec![tenant_a.as_uuid(), tenant_b.as_uuid()])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM tenancy.tenants WHERE id = ANY($1)")
            .bind(vec![tenant_a.as_uuid(), tenant_b.as_uuid()])
            .execute(&pool)
            .await
            .unwrap();
    }
}
