use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::analytics::publish_topic_event,
    contracts::{VerifiedWebhookEvent, WebhookInbox},
    error::database_error,
};

/// Verifies and normalizes wire payloads only; it does not interpret them. A
/// verified webhook is appended to `integration.provider_webhooks` (one
/// row per `(provider_account_id, provider_event_id)` — the unique constraint
/// is the dedup) and, in the same transaction, a `provider.webhook.received`
/// job is published onto `provider_webhooks_queue`. The drain worker
/// (`crate::webhooks::ProviderWebhookWorker`) applies the event through the
/// owning capability. See `migrations/0004_integration.sql`.
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
    async fn record(&self, event: &VerifiedWebhookEvent) -> Result<bool, ApplicationError> {
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

        let webhook_id = Uuid::now_v7();
        let inserted = sqlx::query(
            "INSERT INTO integration.provider_webhooks \
                 (id, store_id, provider_account_id, capability, provider, provider_event_id, \
                  provider_event_type, normalized_event_type, payload) \
             VALUES ($1, $2, $3, $4::integration.provider_capability, $5, $6, $7, $8, $9) \
             ON CONFLICT (provider_account_id, provider_event_id) DO NOTHING",
        )
        .bind(webhook_id)
        .bind(account.1)
        .bind(account.0)
        .bind(&event.capability)
        .bind(&event.provider)
        .bind(&event.provider_event_id)
        .bind(&event.provider_event_type)
        .bind(event.normalized_event_type.as_deref())
        .bind(&event.payload)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected()
            == 1;

        if inserted {
            publish_topic_event(
                &mut transaction,
                "provider.webhook.received",
                json!({ "webhook_id": webhook_id, "store_id": account.1 }),
            )
            .await?;
        }

        transaction.commit().await.map_err(database_error)?;
        Ok(inserted)
    }
}

/// Reads and closes out `integration.provider_webhooks` rows on behalf of
/// the drain worker. Every method scopes itself to one store's rows through
/// the row-level security policy.
#[derive(Clone)]
pub struct PostgresProviderWebhookAudit {
    pool: PgPool,
}

/// One `integration.provider_webhooks` row, as the drain worker needs it.
pub struct ProviderWebhookAuditRow {
    pub store_id: Uuid,
    pub provider_account_id: Uuid,
    pub capability: String,
    pub provider: String,
    pub provider_event_type: String,
    pub normalized_event_type: Option<String>,
    pub payload: Value,
    pub processed_at: Option<OffsetDateTime>,
}

impl PostgresProviderWebhookAudit {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Loads the audited event for `webhook_id`. `store_id` comes from the queue
    /// message so the row-level security policy can be set before the read.
    pub async fn load(
        &self,
        store_id: Uuid,
        webhook_id: Uuid,
    ) -> Result<Option<ProviderWebhookAuditRow>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let row = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                String,
                String,
                String,
                Option<String>,
                Value,
                Option<OffsetDateTime>,
            ),
        >(
            "SELECT store_id, provider_account_id, capability::text, provider, \
                    provider_event_type, normalized_event_type, payload, processed_at \
             FROM integration.provider_webhooks WHERE id = $1",
        )
        .bind(webhook_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(row.map(|row| ProviderWebhookAuditRow {
            store_id: row.0,
            provider_account_id: row.1,
            capability: row.2,
            provider: row.3,
            provider_event_type: row.4,
            normalized_event_type: row.5,
            payload: row.6,
            processed_at: row.7,
        }))
    }

    /// Stamps `processed_at` the first time the event is applied; a redelivery
    /// leaves the original timestamp untouched.
    pub async fn mark_processed(
        &self,
        store_id: Uuid,
        webhook_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query(
            "UPDATE integration.provider_webhooks SET processed_at = $2 \
             WHERE id = $1 AND processed_at IS NULL",
        )
        .bind(webhook_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
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
