use crate::{
    ApplicationError,
    contracts::{ShippingEventJob, ShippingEventQueue},
    error::database_error,
};
use async_trait::async_trait;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresShippingEventRepository {
    pool: PgPool,
}

impl PostgresShippingEventRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_store(
        &self,
        store_id: Uuid,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_store_context(
            &mut transaction,
            chaos_domain::store::StoreId::from_uuid(store_id),
        )
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }
}

#[async_trait]
impl ShippingEventQueue for PostgresShippingEventRepository {
    async fn claim_events(&self, limit: u16) -> Result<Vec<ShippingEventJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, String, Value, i32)>(
            "SELECT id, store_id, event_type, payload, attempts \
             FROM integration.claim_shipping_events($1)",
        )
        .bind(i32::from(limit.clamp(1, 100)))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|(id, store_id, event_type, payload, attempts)| {
            Ok(ShippingEventJob {
                id,
                store_id,
                event_type,
                payload,
                attempts: u32::try_from(attempts)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
            })
        })
        .collect()
    }

    async fn process_event(
        &self,
        job: &ShippingEventJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let order_id = payload_uuid(&job.payload, "order_id")?;
        let shipping_status = match job.event_type.as_str() {
            "shipping.shipped" => "shipped",
            "shipping.delivered" => "delivered",
            "shipping.cancelled" => "cancelled",
            _ => {
                return Err(ApplicationError::Conflict {
                    code: "unsupported_shipping_event",
                    message: "the shipping event type is not supported",
                });
            }
        };
        let provider = payload_string(&job.payload, "provider");
        let provider_reference = payload_string(&job.payload, "provider_reference");
        let tracking_number = payload_string(&job.payload, "tracking_number");
        let tracking_url = payload_string(&job.payload, "tracking_url");
        let mut transaction = self.begin_store(job.store_id).await?;
        let updated = sqlx::query(
            "UPDATE commerce.orders
                SET shipping_status = $3::commerce.order_shipping_status,
                    shipping_provider = COALESCE($4, shipping_provider),
                    shipping_provider_reference = COALESCE($5, shipping_provider_reference),
                    shipping_tracking_number = COALESCE($6, shipping_tracking_number),
                    shipping_tracking_url = COALESCE($7, shipping_tracking_url),
                    updated_at = $8
              WHERE store_id = $1 AND id = $2",
        )
        .bind(job.store_id)
        .bind(order_id)
        .bind(shipping_status)
        .bind(provider)
        .bind(provider_reference)
        .bind(tracking_number)
        .bind(tracking_url)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if updated.rows_affected() == 0 {
            return Err(ApplicationError::NotFound {
                resource: "order",
                id: order_id.to_string(),
            });
        }
        transaction.commit().await.map_err(database_error)
    }

    async fn finish_event(
        &self,
        job_id: Uuid,
        attempts: u32,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (succeeded, failure) = match result {
            Ok(()) => (true, String::new()),
            Err(failure) => (false, failure),
        };
        let finished: Option<bool> =
            sqlx::query_scalar("SELECT integration.finish_event_outbox($1, $2, $3, $4, $5, $6)")
                .bind(job_id)
                .bind(i32::try_from(attempts).unwrap_or(i32::MAX))
                .bind(succeeded)
                .bind(failure)
                .bind(8_i32)
                .bind(now)
                .fetch_one(&self.pool)
                .await
                .map_err(database_error)?;
        if finished == Some(true) {
            Ok(())
        } else {
            Err(ApplicationError::NotFound {
                resource: "shipping_event_job",
                id: job_id.to_string(),
            })
        }
    }
}

fn payload_uuid(payload: &Value, field: &str) -> Result<Uuid, ApplicationError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or(ApplicationError::Conflict {
            code: "invalid_shipping_event",
            message: "shipping event is missing order_id",
        })?
        .parse()
        .map_err(|_| ApplicationError::Conflict {
            code: "invalid_shipping_event",
            message: "shipping event order_id is not a UUID",
        })
}

fn payload_string(payload: &Value, field: &str) -> Option<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
}
