use crate::{
    ApplicationError,
    contracts::{EmailMessage, QueueJob},
    error::database_error,
};
use chaos_domain::store::StorefrontOrigin;
use serde_json::Value;
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresEmailRepository {
    pool: PgPool,
}

impl PostgresEmailRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns `None` when the Order has no contact email yet (the shopper's
    /// payment webhook has not backfilled one). That is a terminal outcome,
    /// not a transient failure: there is nobody to send the confirmation to,
    /// and retrying will not change that once the checkout session itself
    /// has settled without an email.
    pub async fn prepare_order_confirmation(
        &self,
        job: &QueueJob,
    ) -> Result<Option<(String, String, EmailMessage)>, ApplicationError> {
        let order_id = job
            .payload
            .get("aggregate_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| invalid_email_job("aggregate_id"))?;
        let tracking_token = job
            .payload
            .get("tracking_token")
            .and_then(Value::as_str)
            .filter(|value| value.starts_with("ot_") && value.len() > 3)
            .ok_or_else(|| invalid_email_job("tracking_token"))?;
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(job.store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let row =
            sqlx::query_as::<_, (Option<String>, String, i64, String, String, String, String)>(
                "SELECT order_row.contact_email::text, order_row.order_number, \
                    order_row.total_amount_minor, order_row.currency::text, \
                    channel.storefront_origin, \
                    account.provider, account.credential_secret_reference \
             FROM commerce.orders AS order_row \
             INNER JOIN commerce.store_sales_channels AS channel \
               ON channel.store_id = order_row.store_id \
              AND channel.id = order_row.sales_channel_id \
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
        let Some(contact_email) = row.0 else {
            return Ok(None);
        };
        let sender = job
            .payload
            .get("sender")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("orders@chaos.example");
        let tracking_url = order_tracking_url(&row.4, tracking_token)?;
        let text = format!(
            "Your order {} has been confirmed. Total: {} {}. Track your order: {}",
            row.1, row.2, row.3, tracking_url
        );
        Ok(Some((
            row.5,
            row.6,
            EmailMessage {
                from: sender.to_owned(),
                to: contact_email,
                subject: format!("Order {} confirmed", row.1),
                text,
                html: None,
                idempotency_key: format!("order-confirmed-{}", order_id.simple()),
            },
        )))
    }
}

fn order_tracking_url(origin: &str, tracking_token: &str) -> Result<Url, ApplicationError> {
    let origin = StorefrontOrigin::parse(origin.to_owned())
        .map_err(|error| invalid_email_url(error.to_string()))?;
    let mut tracking_url = Url::parse(origin.as_str())
        .map_err(|error| invalid_email_url(error.to_string()))?
        .join("orders/track")
        .map_err(|error| invalid_email_url(error.to_string()))?;
    tracking_url.set_fragment(Some(&format!("token={tracking_token}")));
    Ok(tracking_url)
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

fn invalid_email_url(error: String) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "failed to build order tracking URL: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use super::order_tracking_url;

    #[test]
    fn tracking_url_uses_the_sales_channel_origin() {
        let first = order_tracking_url("https://first.example.test", "ot_first").unwrap();
        let second = order_tracking_url("https://second.example.test/", "ot_second").unwrap();

        assert_eq!(
            first.as_str(),
            "https://first.example.test/orders/track#token=ot_first"
        );
        assert_eq!(
            second.as_str(),
            "https://second.example.test/orders/track#token=ot_second"
        );
    }
}
