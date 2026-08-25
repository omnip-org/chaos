use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{ApplicationError, contracts::ProviderAccountReader, error::database_error};

#[derive(Clone)]
pub struct PostgresIntegrationAccountRepository {
    pool: PgPool,
}

impl PostgresIntegrationAccountRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProviderAccountReader for PostgresIntegrationAccountRepository {
    async fn resolve_webhook_secret(
        &self,
        capability: &str,
        provider: &str,
        provider_account_id: Uuid,
    ) -> Result<Option<(Uuid, String)>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, String)>(
            "SELECT provider_account_id, store_id, secret_reference \
             FROM integration.resolve_webhook_secret_reference(\
                 $1::integration.provider_capability, $2, $3)",
        )
        .bind(capability)
        .bind(provider)
        .bind(provider_account_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)
        .map(|row| row.map(|(_, store_id, reference)| (store_id, reference)))
    }
}
