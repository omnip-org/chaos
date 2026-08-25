use crate::{
    ApplicationError,
    contracts::{EmailMessage, QueueJob},
    error::database_error,
};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresEmailRepository {
    pool: PgPool,
}

impl PostgresEmailRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn prepare_order_confirmation(
        &self,
        job: &QueueJob,
    ) -> Result<(String, String, EmailMessage), ApplicationError> {
        let order_id = job
            .payload
            .get("aggregate_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| invalid_email_job("aggregate_id"))?;
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(job.store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let row = sqlx::query_as::<_, (String, String, i64, String, String, String)>(
            "SELECT order_row.contact_email::text, order_row.order_number, \
                    order_row.total_amount_minor, order_row.currency::text, \
                    account.provider, account.credential_secret_reference \
             FROM commerce.orders AS order_row \
             INNER JOIN integration.provider_accounts AS account \
               ON account.store_id = order_row.store_id \
              AND account.capability = 'email' \
              AND account.enabled \
              AND account.credential_secret_reference IS NOT NULL \
             WHERE order_row.store_id = $1 AND order_row.id = $2 \
             ORDER BY account.id LIMIT 1",
        )
        .bind(job.store_id)
        .bind(order_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(email_provider_unavailable)?;
        transaction.commit().await.map_err(database_error)?;
        let sender = job
            .payload
            .get("sender")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("orders@chaos.example");
        let text = format!(
            "Your order {} has been confirmed. Total: {} {}.",
            row.1, row.2, row.3
        );
        Ok((
            row.4,
            row.5,
            EmailMessage {
                from: sender.to_owned(),
                to: row.0,
                subject: format!("Order {} confirmed", row.1),
                text,
                html: None,
                idempotency_key: format!("order-confirmed-{}", order_id.simple()),
            },
        ))
    }
}

fn invalid_email_job(field: &'static str) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("email outbox payload is missing {field}"))
}

fn email_provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "email_provider_unavailable",
        message: "no configured Email provider account is available",
    }
}
