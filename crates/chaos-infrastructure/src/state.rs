use std::time::Duration;

use anyhow::Context;
use redis::{AsyncCommands, Client as RedisClient};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{config::Settings, tenancy::TenantTransaction};
use chaos_domain::tenancy::TenantId;

#[derive(Clone)]
pub struct AppState {
    postgres: PgPool,
    redis: RedisClient,
    pub dependency_timeout: Duration,
}

impl AppState {
    pub fn new(settings: &Settings) -> anyhow::Result<Self> {
        let runtime_role = settings.database_runtime_role.clone();
        let postgres = PgPoolOptions::new()
            .max_connections(settings.database_max_connections)
            .acquire_timeout(settings.database_acquire_timeout)
            .after_connect(move |connection, _metadata| {
                let runtime_role = runtime_role.clone();
                Box::pin(async move {
                    if let Some(role) = runtime_role {
                        let statement = format!("SET ROLE {role}");
                        // Settings constrains the identifier to [a-z_][a-z0-9_]*.
                        sqlx::query(sqlx::AssertSqlSafe(statement))
                            .execute(&mut *connection)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect_lazy(&settings.database_url)
            .context("invalid DATABASE_URL")?;
        let redis = RedisClient::open(settings.redis_url.as_str()).context("invalid REDIS_URL")?;

        Ok(Self {
            postgres,
            redis,
            dependency_timeout: settings.dependency_timeout,
        })
    }

    pub async fn begin_tenant_transaction(
        &self,
        tenant_id: TenantId,
    ) -> anyhow::Result<TenantTransaction<'_>> {
        TenantTransaction::begin(&self.postgres, tenant_id).await
    }

    pub async fn check_dependencies(&self) -> anyhow::Result<()> {
        let postgres = async {
            sqlx::query("SELECT 1")
                .execute(&self.postgres)
                .await
                .context("PostgreSQL readiness check failed")?;
            Ok::<_, anyhow::Error>(())
        };
        let redis = async {
            let mut connection = self
                .redis
                .get_multiplexed_async_connection()
                .await
                .context("Redis connection failed")?;
            let pong: String = connection.ping().await.context("Redis PING failed")?;
            anyhow::ensure!(pong == "PONG", "unexpected Redis PING response");
            Ok::<_, anyhow::Error>(())
        };

        tokio::try_join!(postgres, redis)?;
        Ok(())
    }
}
