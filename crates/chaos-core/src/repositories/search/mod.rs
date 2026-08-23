use crate::ApplicationError;
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
        let processed: i64 = sqlx::query_scalar("SELECT commerce.process_events($1, $2)")
            .bind(i32::from(limit.clamp(1, 100)))
            .bind(now)
            .fetch_one(&self.pool)
            .await
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        u64::try_from(processed).map_err(|error| ApplicationError::Unexpected(error.into()))
    }
}
