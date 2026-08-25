use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    ApplicationError,
    contracts::{VerifiedWebhookEvent, WebhookInbox},
    error::database_error,
};

/// Canonical persistence for every verified provider webhook. Capability
/// adapters only verify and normalize wire payloads; they do not own inbox
/// rows, idempotency, or queue delivery.
#[derive(Clone)]
pub struct PostgresIntegrationWebhookRepository {
    pool: PgPool,
}

impl PostgresIntegrationWebhookRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl WebhookInbox for PostgresIntegrationWebhookRepository {
    async fn record(&self, event: VerifiedWebhookEvent) -> Result<bool, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let account = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT provider_account_id, store_id \
             FROM integration.resolve_provider_account(\
                 $1::integration.provider_capability, $2, $3)",
        )
        .bind(&event.capability)
        .bind(&event.provider)
        .bind(event.provider_account_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(integration_provider_unavailable)?;

        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(account.1.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;

        let result = sqlx::query(
            "INSERT INTO integration.provider_webhook_inbox \
             (id, store_id, provider_account_id, capability, provider, provider_event_id, \
              provider_event_type, normalized_event_type, payload, aggregate_type, aggregate_id, verified_at) \
             VALUES ($1, $2, $3, $4::integration.provider_capability, $5, $6, $7, $8, $9, $10, $11, $12) \
             ON CONFLICT (provider_account_id, provider_event_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(account.1)
        .bind(account.0)
        .bind(&event.capability)
        .bind(&event.provider)
        .bind(&event.provider_event_id)
        .bind(&event.provider_event_type)
        .bind(event.normalized_event_type.as_deref())
        .bind(&event.payload)
        .bind(event.aggregate_type.as_deref())
        .bind(event.aggregate_id)
        .bind(event.verified_at)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;

        transaction.commit().await.map_err(database_error)?;
        Ok(result.rows_affected() == 1)
    }
}

fn integration_provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "integration_provider_account_unavailable",
        message: "the provider account is unavailable or disabled",
    }
}
