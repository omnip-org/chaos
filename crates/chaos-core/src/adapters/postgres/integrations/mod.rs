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
             FROM chaos_integration.claim_topic_queue($1, $2)",
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
        sqlx::query("SELECT chaos_integration.finish_topic_event($1, $2, $3, $4, $5)")
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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::PostgresIntegrationQueue;
    use crate::contracts::IntegrationQueue;

    /// `order.payment.completed` is the only routing key bound to two queues, so
    /// it exercises the fan-out loop in `chaos_integration.publish_topic_event` that
    /// replaced pgmq's own `send_topic` / `bind_topic` (unavailable before pgmq
    /// 1.11).
    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn publish_fans_a_topic_out_to_every_bound_queue() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();

        let marker = Uuid::now_v7();
        let delivered: i32 = sqlx::query_scalar(
            "SELECT chaos_integration.publish_topic_event('order.payment.completed', $1)",
        )
        .bind(json!({ "store_id": marker, "order_id": marker }))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(delivered, 2, "expected fan-out to both bound queues");

        let queue = PostgresIntegrationQueue::new(pool);
        for queue_name in ["analytics_capi_queue", "notification_email_queue"] {
            let jobs = queue.claim_topic(queue_name, 100).await.unwrap();
            let mine = jobs
                .into_iter()
                .find(|job| {
                    job.payload.get("order_id").and_then(|value| value.as_str())
                        == Some(marker.to_string().as_str())
                })
                .unwrap_or_else(|| panic!("{queue_name} did not receive the published event"));
            assert_eq!(mine.routing_key, "order.payment.completed");
            queue
                .finish_topic(queue_name, mine.msg_id, mine.attempts, Ok(()))
                .await
                .unwrap();
        }
    }
}
