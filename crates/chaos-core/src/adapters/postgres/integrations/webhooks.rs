use async_trait::async_trait;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::analytics::publish_commerce_event,
    contracts::{VerifiedWebhookEvent, WebhookInbox},
    error::database_error,
};

/// Verifies and normalizes wire payloads only; it does not own dedup or
/// queue delivery. A verified webhook publishes directly to
/// `webhook.<capability>` (topic-routed, one queue per consumer — see
/// `migrations/0004_integration.sql`) in the same transaction that
/// validates the provider account, so a rolled-back transaction never
/// delivers a message a consumer would act on.
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
    async fn record(&self, event: VerifiedWebhookEvent) -> Result<(), ApplicationError> {
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

        let routing_key = format!("webhook.{}", event.capability);
        publish_commerce_event(
            &mut transaction,
            &routing_key,
            json!({
                "store_id": account.1,
                "provider_account_id": account.0,
                "provider": event.provider,
                "provider_event_id": event.provider_event_id,
                "provider_event_type": event.provider_event_type,
                "normalized_event_type": event.normalized_event_type,
                "payload": event.payload,
                "verified_at": event.verified_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            }),
        )
        .await?;

        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }
}

fn integration_provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "integration_provider_account_unavailable",
        message: "the provider account is unavailable or disabled",
    }
}
