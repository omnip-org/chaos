use crate::{
    ApplicationError,
    contracts::{IntegrationQueue, MAX_INTEGRATION_ATTEMPTS, QueueJob},
    error::database_error,
};
use async_trait::async_trait;
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

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
    async fn claim_outbox(&self, limit: u16) -> Result<Vec<QueueJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, String, Value, i32)>(
            "SELECT id, store_id, event_type, payload, attempts \
             FROM commerce.claim_event_outbox($1)",
        )
        .bind(i32::from(limit.clamp(1, 100)))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(queue_job)
        .collect()
    }

    async fn claim_webhooks(&self, limit: u16) -> Result<Vec<QueueJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, String, String, Value, i32)>(
            "SELECT id, store_id, provider, event_type, payload, attempts \
             FROM commerce.claim_webhook_events($1)",
        )
        .bind(i32::from(limit.clamp(1, 100)))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| queue_job((row.0, row.1, row.3, row.4, row.5)))
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
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        finish_webhook_job(&self.pool, job_id, attempts, result, now).await
    }
}

async fn finish_webhook_job(
    pool: &PgPool,
    job_id: Uuid,
    attempts: u32,
    result: Result<(), String>,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let (succeeded, failure) = finish_result(result);
    let finished: Option<bool> =
        sqlx::query_scalar("SELECT commerce.finish_webhook_event($1, $2, $3, $4, $5, $6)")
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

fn queue_job(row: (Uuid, Uuid, String, Value, i32)) -> Result<QueueJob, ApplicationError> {
    Ok(QueueJob {
        id: row.0,
        store_id: row.1,
        event_type: row.2,
        payload: row.3,
        attempts: u32::try_from(row.4)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
    })
}

fn queue_job_not_found() -> ApplicationError {
    ApplicationError::Conflict {
        code: "queue_lease_lost",
        message: "the queue job is no longer leased by this worker",
    }
}
