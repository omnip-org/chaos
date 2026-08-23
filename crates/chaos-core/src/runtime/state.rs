use std::time::Duration;

use anyhow::Context;
use redis::{AsyncCommands, Client as RedisClient};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::{
    adapters::postgres::database::store_context::StoreTransaction, runtime::config::Settings,
};
use chaos_domain::store::StoreId;

#[derive(Clone)]
pub struct AppState {
    postgres: PgPool,
    identity_postgres: PgPool,
    analytics_postgres: PgPool,
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
        let analytics_runtime_role = settings.database_runtime_role.clone();
        let analytics_statement_timeout = settings.database_analytics_statement_timeout;
        let analytics_postgres = PgPoolOptions::new()
            .max_connections(settings.database_analytics_max_connections)
            .acquire_timeout(settings.database_acquire_timeout)
            .after_connect(move |connection, _metadata| {
                let runtime_role = analytics_runtime_role.clone();
                Box::pin(async move {
                    if let Some(role) = runtime_role {
                        let statement = format!("SET ROLE {role}");
                        sqlx::query(sqlx::AssertSqlSafe(statement))
                            .execute(&mut *connection)
                            .await?;
                    }
                    sqlx::query("SET default_transaction_read_only = on")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET lock_timeout = '100ms'")
                        .execute(&mut *connection)
                        .await?;
                    let timeout = format!(
                        "SET statement_timeout = '{}ms'",
                        analytics_statement_timeout.as_millis()
                    );
                    sqlx::query(sqlx::AssertSqlSafe(timeout))
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_lazy(&settings.database_url)
            .context("invalid analytics DATABASE_URL")?;
        let identity_role = settings.database_identity_role.clone();
        let identity_postgres = PgPoolOptions::new()
            .max_connections(settings.database_identity_max_connections)
            .acquire_timeout(settings.database_acquire_timeout)
            .after_connect(move |connection, _metadata| {
                let identity_role = identity_role.clone();
                Box::pin(async move {
                    if let Some(role) = identity_role {
                        let statement = format!("SET ROLE {role}");
                        // Settings constrains the identifier to [a-z_][a-z0-9_]*.
                        sqlx::query(sqlx::AssertSqlSafe(statement))
                            .execute(&mut *connection)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect_lazy(&settings.database_identity_url)
            .context("invalid DATABASE_IDENTITY_URL")?;

        Ok(Self {
            postgres,
            identity_postgres,
            analytics_postgres,
            redis,
            dependency_timeout: settings.dependency_timeout,
        })
    }

    pub fn identity_pool(&self) -> PgPool {
        self.identity_postgres.clone()
    }

    pub fn runtime_pool(&self) -> PgPool {
        self.postgres.clone()
    }

    pub fn redis_client(&self) -> RedisClient {
        self.redis.clone()
    }

    pub fn analytics_pool(&self) -> PgPool {
        self.analytics_postgres.clone()
    }

    pub async fn begin_store_transaction(
        &self,
        store_id: StoreId,
    ) -> anyhow::Result<StoreTransaction<'_>> {
        StoreTransaction::begin(&self.postgres, store_id).await
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
        let identity_postgres = async {
            sqlx::query("SELECT 1")
                .execute(&self.identity_postgres)
                .await
                .context("identity PostgreSQL readiness check failed")?;
            Ok::<_, anyhow::Error>(())
        };
        let analytics_postgres = async {
            sqlx::query("SELECT 1")
                .execute(&self.analytics_postgres)
                .await
                .context("Analytics PostgreSQL readiness check failed")?;
            Ok::<_, anyhow::Error>(())
        };

        tokio::try_join!(postgres, identity_postgres, analytics_postgres, redis)?;
        Ok(())
    }
}
