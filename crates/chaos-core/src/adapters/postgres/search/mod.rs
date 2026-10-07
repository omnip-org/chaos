use crate::{
    ApplicationError,
    contracts::{MAX_INTEGRATION_ATTEMPTS, SEARCH_INDEX_QUEUE},
};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;

#[derive(Clone)]
pub struct PostgresSearchIndexer {
    pool: PgPool,
}

impl PostgresSearchIndexer {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn run_batch(
        &self,
        limit: u16,
        now: OffsetDateTime,
    ) -> Result<u64, ApplicationError> {
        let (processed, failures): (i64, Value) = sqlx::query_as(
            "SELECT processed_count, failures \
             FROM chaos_commerce.process_search_index_events_detailed($1, $2, $3)",
        )
        .bind(i32::from(limit.clamp(1, 100)))
        .bind(MAX_INTEGRATION_ATTEMPTS)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        let failed = log_search_failures(&failures);
        let handled = processed
            .checked_add(i64::try_from(failed).unwrap_or(i64::MAX))
            .ok_or_else(|| ApplicationError::Unexpected(anyhow::anyhow!("batch size overflow")))?;
        u64::try_from(handled).map_err(|error| ApplicationError::Unexpected(error.into()))
    }
}

fn log_search_failures(failures: &Value) -> usize {
    let Some(failures) = failures.as_array() else {
        tracing::error!(
            queue = SEARCH_INDEX_QUEUE,
            failures = %failures,
            "search index batch returned malformed failure details"
        );
        return 0;
    };
    for failure in failures {
        let msg_id = failure.get("msg_id").and_then(Value::as_i64);
        let attempts = failure.get("attempts").and_then(Value::as_i64);
        let store_id = failure.get("store_id").and_then(Value::as_str);
        let product_id = failure.get("product_id").and_then(Value::as_str);
        let error = failure
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown search indexing error");
        if failure
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            tracing::error!(
                queue = SEARCH_INDEX_QUEUE,
                ?msg_id,
                ?attempts,
                ?store_id,
                ?product_id,
                error,
                "search index event archived after delivery failure"
            );
        } else {
            tracing::warn!(
                queue = SEARCH_INDEX_QUEUE,
                ?msg_id,
                ?attempts,
                ?store_id,
                ?product_id,
                error,
                "search index event failed; retry scheduled"
            );
        }
    }
    failures.len()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use sqlx::postgres::PgPoolOptions;
    use time::OffsetDateTime;
    use uuid::Uuid;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn malformed_search_event_is_returned_with_archive_details() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .unwrap();
        let marker = Uuid::now_v7();

        let delivered: i32 = sqlx::query_scalar(
            "SELECT chaos_integration.publish_topic_event('product.updated', $1)",
        )
        .bind(json!({
            "store_id": "not-a-uuid",
            "product_id": marker,
        }))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(delivered, 1);

        let (_processed, failures): (i64, Value) = sqlx::query_as(
            "SELECT processed_count, failures \
             FROM chaos_commerce.process_search_index_events_detailed($1, $2, $3)",
        )
        .bind(100_i32)
        .bind(1_i32)
        .bind(OffsetDateTime::now_utc())
        .fetch_one(&pool)
        .await
        .unwrap();
        let marker = marker.to_string();
        let failure = failures
            .as_array()
            .unwrap()
            .iter()
            .find(|failure| failure.get("product_id").and_then(Value::as_str) == Some(&marker))
            .expect("malformed event must be returned as a failure");

        assert_eq!(failure.get("archived").and_then(Value::as_bool), Some(true));
        assert!(
            failure
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty())
        );
    }
}
