use crate::{
    ApplicationError,
    contracts::{IntegrationQueue, MAX_INTEGRATION_ATTEMPTS, QueueJob, WebhookProcessingResult},
    error::database_error,
};
use async_trait::async_trait;
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

mod accounts;
mod email;
mod shipping;
mod webhooks;
pub use accounts::PostgresIntegrationAccountRepository;
pub use email::PostgresEmailRepository;
pub(crate) use email::{EmailBrandWrite, EmailProviderAccountWrite};
pub use shipping::PostgresShippingRepository;
pub use webhooks::PostgresIntegrationWebhookRepository;

/// PostgreSQL-backed leasing for integration outbox and webhook jobs.
/// Provider-specific payload interpretation stays in the owning application
/// service; this type only knows the durable queue contract.
pub struct PostgresIntegrationQueue {
    pool: PgPool,
}

impl PostgresIntegrationQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl IntegrationQueue for PostgresIntegrationQueue {
    async fn claim_outbox(
        &self,
        queue_name: &str,
        limit: u16,
    ) -> Result<Vec<QueueJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, String, Value, i32)>(
            "SELECT id, store_id, internal_event_type, payload, attempts \
             FROM integration.claim_event_outbox($1, $2)",
        )
        .bind(queue_name)
        .bind(i32::from(limit.clamp(1, 100)))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            let internal_event_type = Some(row.2.clone());
            queue_job(
                row,
                queue_name.to_owned(),
                QueueJobMetadata {
                    provider_account_id: None,
                    capability: None,
                    provider: None,
                    internal_event_type,
                    provider_event_type: None,
                    normalized_event_type: None,
                },
            )
        })
        .collect()
    }

    async fn claim_webhooks(
        &self,
        capability: &str,
        limit: u16,
    ) -> Result<Vec<QueueJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, String, String, Option<String>, Value, i32)>(
            "SELECT id, store_id, provider_account_id, capability::text, provider, \
                    provider_event_type, normalized_event_type, payload, attempts \
             FROM integration.claim_provider_webhook_inbox($1::integration.provider_capability, $2)",
        )
        .bind(capability)
        .bind(i32::from(limit.clamp(1, 100)))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            queue_job(
                (row.0, row.1, row.5.clone(), row.7, row.8),
                "chaos_webhooks".into(),
                QueueJobMetadata {
                    provider_account_id: Some(row.2),
                    capability: Some(row.3),
                    provider: Some(row.4),
                    internal_event_type: None,
                    provider_event_type: Some(row.5),
                    normalized_event_type: row.6,
                },
            )
        })
        .collect()
    }

    async fn finish_outbox(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        finish_outbox_job(&self.pool, job_id, attempts, result, now).await
    }

    async fn finish_webhook(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: WebhookProcessingResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        finish_webhook_job(&self.pool, job_id, attempts, result, now).await
    }
}

async fn finish_webhook_job(
    pool: &PgPool,
    job_id: Uuid,
    attempts: u32,
    result: WebhookProcessingResult,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let (outcome, failure) = webhook_result(result);
    let finished: Option<bool> = sqlx::query_scalar(
        "SELECT integration.finish_provider_webhook(\
                $1, $2, $3::integration.webhook_processing_status, $4, $5, $6)",
    )
    .bind(job_id)
    .bind(i32::try_from(attempts).unwrap_or(i32::MAX))
    .bind(outcome)
    .bind(&failure)
    .bind(MAX_INTEGRATION_ATTEMPTS)
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(database_error)?;
    if finished == Some(true) {
        Ok(())
    } else {
        Err(queue_job_not_found())
    }
}

async fn finish_outbox_job(
    pool: &PgPool,
    job_id: Uuid,
    attempts: u32,
    result: Result<(), String>,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let (succeeded, failure) = finish_result(result);
    let finished: Option<bool> =
        sqlx::query_scalar("SELECT integration.finish_event_outbox($1, $2, $3, $4, $5, $6)")
            .bind(job_id)
            .bind(i32::try_from(attempts).unwrap_or(i32::MAX))
            .bind(succeeded)
            .bind(&failure)
            .bind(MAX_INTEGRATION_ATTEMPTS)
            .bind(now)
            .fetch_one(pool)
            .await
            .map_err(database_error)?;
    if finished == Some(true) {
        Ok(())
    } else {
        Err(queue_job_not_found())
    }
}

fn finish_result(result: Result<(), String>) -> (bool, String) {
    match result {
        Ok(()) => (true, String::new()),
        Err(failure) => (false, failure),
    }
}

fn webhook_result(result: WebhookProcessingResult) -> (&'static str, String) {
    match result {
        WebhookProcessingResult::Processed => ("processed", String::new()),
        WebhookProcessingResult::Unsupported { reason } => ("unsupported", reason),
        WebhookProcessingResult::Failed { reason } => ("failed", reason),
    }
}

struct QueueJobMetadata {
    provider_account_id: Option<Uuid>,
    capability: Option<String>,
    provider: Option<String>,
    internal_event_type: Option<String>,
    provider_event_type: Option<String>,
    normalized_event_type: Option<String>,
}

fn queue_job(
    row: (Uuid, Uuid, String, Value, i32),
    queue_name: String,
    metadata: QueueJobMetadata,
) -> Result<QueueJob, ApplicationError> {
    let inferred_capability = metadata.capability.or_else(|| match queue_name.as_str() {
        "chaos_payment_commands" => Some("payment".to_owned()),
        "chaos_email_commands" => Some("email".to_owned()),
        "chaos_shipping_commands" => Some("shipping".to_owned()),
        _ => None,
    });
    let inferred_provider = metadata.provider.or_else(|| {
        row.3
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    Ok(QueueJob {
        id: row.0,
        store_id: row.1,
        queue_name,
        internal_event_type: metadata.internal_event_type,
        provider_event_type: metadata.provider_event_type,
        normalized_event_type: metadata.normalized_event_type,
        payload: row.3,
        attempts: u32::try_from(row.4)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        provider_account_id: metadata.provider_account_id,
        capability: inferred_capability,
        provider: inferred_provider,
    })
}

fn queue_job_not_found() -> ApplicationError {
    ApplicationError::Conflict {
        code: "queue_lease_lost",
        message: "the queue job is no longer leased by this worker",
    }
}
