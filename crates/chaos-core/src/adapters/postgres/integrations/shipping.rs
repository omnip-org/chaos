use crate::{
    ApplicationError,
    contracts::{QueueJob, ShippingCommand, ShippingOperation, ShippingResult},
    error::database_error,
};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresShippingRepository {
    pool: PgPool,
}

impl PostgresShippingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn prepare_shipped_command(
        &self,
        job: &QueueJob,
    ) -> Result<(String, ShippingCommand), ApplicationError> {
        let fulfillment_id = job
            .payload
            .get("fulfillment_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| invalid_shipping_job("fulfillment_id"))?;
        let order_id = job
            .payload
            .get("order_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| invalid_shipping_job("order_id"))?;
        let account_id = job
            .payload
            .get("shipping_provider_account_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| invalid_shipping_job("shipping_provider_account_id"))?;
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(job.store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let provider: String = sqlx::query_scalar(
            "SELECT provider FROM integration.provider_accounts \
             WHERE id = $1 AND store_id = $2 AND capability = 'shipping' AND enabled",
        )
        .bind(account_id)
        .bind(job.store_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(shipping_provider_unavailable)?;
        transaction.commit().await.map_err(database_error)?;
        let operation = match job
            .payload
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("shipped")
        {
            "shipped" => ShippingOperation::Shipped,
            _ => return Err(invalid_shipping_job("operation")),
        };
        Ok((
            provider,
            ShippingCommand {
                operation,
                provider_account_id: account_id,
                order_id,
                fulfillment_id,
                tracking_number: job
                    .payload
                    .get("tracking_number")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                tracking_url: job
                    .payload
                    .get("tracking_url")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
        ))
    }

    pub async fn record_result(
        &self,
        job: &QueueJob,
        result: &ShippingResult,
        now: time::OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if result.provider_reference_id.is_none()
            && result.tracking_number.is_none()
            && result.tracking_url.is_none()
        {
            return Ok(());
        }
        if result
            .provider_reference_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 255)
        {
            return Err(ApplicationError::Unavailable {
                service: "shipping",
                source: anyhow::anyhow!("Shipping provider returned an invalid reference"),
            });
        }
        let order_id = job
            .payload
            .get("order_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| invalid_shipping_job("order_id"))?;
        let account_id = job
            .payload
            .get("shipping_provider_account_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| invalid_shipping_job("shipping_provider_account_id"))?;
        let fulfillment_id = job
            .payload
            .get("fulfillment_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| invalid_shipping_job("fulfillment_id"))?;
        if result
            .tracking_number
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 255)
            || result.tracking_url.as_deref().is_some_and(|value| {
                value.len() < 9 || value.len() > 2048 || !value.starts_with("https://")
            })
        {
            return Err(ApplicationError::Unavailable {
                service: "shipping",
                source: anyhow::anyhow!("Shipping provider returned invalid tracking data"),
            });
        }
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(job.store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        if result.tracking_number.is_some() || result.tracking_url.is_some() {
            let updated = sqlx::query(
                "UPDATE commerce.order_fulfillments \
                 SET tracking_number = COALESCE($3, tracking_number), \
                     tracking_url = COALESCE($4, tracking_url), updated_at = $5 \
                 WHERE store_id = $1 AND id = $2 AND order_id = $6 \
                   AND shipping_provider_account_id = $7",
            )
            .bind(job.store_id)
            .bind(fulfillment_id)
            .bind(result.tracking_number.as_deref())
            .bind(result.tracking_url.as_deref())
            .bind(now)
            .bind(order_id)
            .bind(account_id)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
            .rows_affected();
            if updated != 1 {
                transaction.rollback().await.map_err(database_error)?;
                return Err(ApplicationError::Conflict {
                    code: "shipping_result_mismatch",
                    message: "the Shipping result does not match the Fulfillment",
                });
            }
        }
        let Some(provider_reference_id) = result.provider_reference_id.as_deref() else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        };
        let updated = sqlx::query(
            "UPDATE commerce.orders \
             SET shipping_provider_reference_id = COALESCE(shipping_provider_reference_id, $3), \
                 updated_at = CASE WHEN shipping_provider_reference_id IS NULL THEN $4 ELSE updated_at END \
             WHERE store_id = $1 AND id = $2 AND shipping_provider_account_id = $5",
        )
        .bind(job.store_id)
        .bind(order_id)
        .bind(provider_reference_id)
        .bind(now)
        .bind(account_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        if updated != 1 {
            transaction.rollback().await.map_err(database_error)?;
            return Err(ApplicationError::Conflict {
                code: "shipping_provider_reference_mismatch",
                message: "the Shipping result does not match the Order",
            });
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }
}

fn invalid_shipping_job(field: &'static str) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "shipping outbox payload is missing {field}"
    ))
}

fn shipping_provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "shipping_provider_unavailable",
        message: "the Shipping provider account is unavailable",
    }
}
