use std::time::Duration;

use anyhow::Context;
use redis::{AsyncCommands, Client as RedisClient};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::runtime::config::Settings;

#[derive(Clone)]
pub struct AppState {
    postgres: PgPool,
    identity_postgres: PgPool,
    redis: RedisClient,
    runtime_role: String,
    identity_role: String,
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
                    let statement = format!("SET ROLE {runtime_role}");
                    // Settings constrains the identifier to [a-z_][a-z0-9_]*.
                    sqlx::query(sqlx::AssertSqlSafe(statement))
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_lazy(&settings.database_url)
            .context("invalid DATABASE_URL")?;
        let redis = RedisClient::open(settings.redis_url.as_str()).context("invalid REDIS_URL")?;
        let identity_role = settings.database_identity_role.clone();
        let identity_postgres = PgPoolOptions::new()
            .max_connections(settings.database_identity_max_connections)
            .acquire_timeout(settings.database_acquire_timeout)
            .after_connect(move |connection, _metadata| {
                let identity_role = identity_role.clone();
                Box::pin(async move {
                    let statement = format!("SET ROLE {identity_role}");
                    // Settings constrains the identifier to [a-z_][a-z0-9_]*.
                    sqlx::query(sqlx::AssertSqlSafe(statement))
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_lazy(&settings.database_identity_url)
            .context("invalid DATABASE_IDENTITY_URL")?;

        Ok(Self {
            postgres,
            identity_postgres,
            redis,
            runtime_role: settings.database_runtime_role.clone(),
            identity_role: settings.database_identity_role.clone(),
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

    pub async fn check_dependencies(&self) -> anyhow::Result<()> {
        let postgres = async {
            let (current_user, rolsuper, rolbypassrls, owns_data_table): (
                String,
                bool,
                bool,
                bool,
            ) = sqlx::query_as(
                "SELECT current_user, role.rolsuper, role.rolbypassrls,
                        EXISTS (
                            SELECT 1
                            FROM pg_class AS relation
                            INNER JOIN pg_namespace AS namespace
                               ON namespace.oid = relation.relnamespace
                            WHERE namespace.nspname = 'commerce'
                              AND relation.relname = 'orders'
                              AND relation.relowner = role.oid
                        )
                   FROM pg_roles AS role
                  WHERE role.rolname = current_user",
            )
            .fetch_one(&self.postgres)
            .await
            .context("PostgreSQL runtime role check failed")?;
            anyhow::ensure!(
                current_user == self.runtime_role && !rolsuper && !rolbypassrls && !owns_data_table,
                "PostgreSQL runtime connection is not using the expected non-owner role"
            );
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
            let (current_user, rolsuper, rolbypassrls, owns_data_table): (
                String,
                bool,
                bool,
                bool,
            ) = sqlx::query_as(
                "SELECT current_user, role.rolsuper, role.rolbypassrls,
                        EXISTS (
                            SELECT 1
                            FROM pg_class AS relation
                            INNER JOIN pg_namespace AS namespace
                               ON namespace.oid = relation.relnamespace
                            WHERE namespace.nspname = 'identity'
                              AND relation.relname = 'users'
                              AND relation.relowner = role.oid
                        )
                   FROM pg_roles AS role
                  WHERE role.rolname = current_user",
            )
            .fetch_one(&self.identity_postgres)
            .await
            .context("identity PostgreSQL role check failed")?;
            anyhow::ensure!(
                current_user == self.identity_role
                    && !rolsuper
                    && !rolbypassrls
                    && !owns_data_table,
                "identity PostgreSQL connection is not using the expected non-owner role"
            );
            sqlx::query("SELECT 1")
                .execute(&self.identity_postgres)
                .await
                .context("identity PostgreSQL readiness check failed")?;
            Ok::<_, anyhow::Error>(())
        };
        tokio::try_join!(postgres, identity_postgres, redis)?;
        Ok(())
    }
}
