use crate::{
    ApplicationError,
    contracts::{IntegrationQueue, MAX_INTEGRATION_ATTEMPTS, TopicEventJob},
    error::database_error,
};
use async_trait::async_trait;
use serde_json::Value;
use sqlx::PgPool;

mod accounts;
mod email;
mod webhooks;
pub use accounts::PostgresIntegrationAccountRepository;
pub use email::PostgresEmailRepository;
pub(crate) use email::{EmailBrandWrite, EmailProviderAccountWrite};
pub use webhooks::{
    PostgresIntegrationWebhookRepository, PostgresProviderWebhookAudit, ProviderWebhookAuditRow,
};

/// PostgreSQL-backed leasing for every topic-routed queue. Provider-specific
/// payload interpretation stays in the owning application service; this
/// type only knows the durable queue contract.
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
    async fn claim_topic(
        &self,
        queue_name: &str,
        limit: u16,
    ) -> Result<Vec<TopicEventJob>, ApplicationError> {
        sqlx::query_as::<_, (i64, Value, i32, String)>(
            "SELECT msg_id, payload, attempts, routing_key \
             FROM integration.claim_topic_queue($1, $2)",
        )
        .bind(queue_name)
        .bind(i32::from(limit.clamp(1, 100)))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|(msg_id, payload, attempts, routing_key)| {
            Ok(TopicEventJob {
                msg_id,
                payload,
                attempts: u32::try_from(attempts)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
                routing_key,
            })
        })
        .collect()
    }

    async fn finish_topic(
        &self,
        queue_name: &str,
        msg_id: i64,
        attempts: u32,
        result: Result<(), String>,
    ) -> Result<(), ApplicationError> {
        let succeeded = result.is_ok();
        sqlx::query("SELECT integration.finish_topic_event($1, $2, $3, $4, $5)")
            .bind(queue_name)
            .bind(msg_id)
            .bind(i32::try_from(attempts).unwrap_or(i32::MAX))
            .bind(succeeded)
            .bind(MAX_INTEGRATION_ATTEMPTS)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }
}
