use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        EmailDeliveryFailure, EmailDeliveryJob, EmailDeliveryRepository, VerifiedEmailWebhook,
    },
};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresEmailDeliveryRepository {
    pool: PgPool,
}

impl PostgresEmailDeliveryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl EmailDeliveryRepository for PostgresEmailDeliveryRepository {
    async fn claim(&self, limit: u16) -> Result<Vec<EmailDeliveryJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, String, String, i32, Value, String, i32)>(
            "SELECT id, store_id, recipient_email, template_key, \
                    template_version, template_payload, provider, attempts \
             FROM integration.claim_email_deliveries($1)",
        )
        .bind(i32::from(limit.clamp(1, 100)))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            Ok(EmailDeliveryJob {
                id: row.0,
                store_id: row.1,
                recipient_email: row.2,
                template_key: row.3,
                template_version: u32::try_from(row.4)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
                template_payload: row.5,
                provider: row.6,
                attempts: u32::try_from(row.7)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
            })
        })
        .collect()
    }

    async fn finish(
        &self,
        delivery_id: Uuid,
        attempts: u32,
        result: Result<chaos_application::ports::EmailDelivery, EmailDeliveryFailure>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (succeeded, retryable, provider_message_id, failure) = match result {
            Ok(delivery) => (
                true,
                false,
                Some(delivery.provider_message_id),
                String::new(),
            ),
            Err(failure) => (false, failure.retryable, None, failure.message),
        };
        let finished: Option<bool> = sqlx::query_scalar(
            "SELECT integration.finish_email_delivery($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(delivery_id)
        .bind(i32::try_from(attempts).unwrap_or(i32::MAX))
        .bind(succeeded)
        .bind(retryable)
        .bind(provider_message_id)
        .bind(failure)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?;
        if finished == Some(true) {
            Ok(())
        } else {
            Err(ApplicationError::Conflict {
                code: "email_delivery_not_pending",
                message: "The email delivery is no longer pending",
            })
        }
    }

    async fn record_webhook(&self, event: &VerifiedEmailWebhook) -> Result<bool, ApplicationError> {
        sqlx::query_scalar::<_, bool>(
            "SELECT integration.record_resend_webhook($1, $2, $3, $4, $5)",
        )
        .bind(&event.provider_event_id)
        .bind(&event.provider_message_id)
        .bind(&event.provider_event_type)
        .bind(&event.payload)
        .bind(event.received_at)
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)
    }
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use chaos_application::ports::{EmailDelivery, EmailDeliveryRepository};
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use time::Duration;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn delivery_queue_webhooks_and_suppressions_are_recoverable_and_isolated() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("owner pool");
        let runtime_pool = PgPoolOptions::new()
            .max_connections(4)
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("SET ROLE chaos_runtime")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .expect("runtime pool");
        sqlx::query(
            "TRUNCATE integration.webhook_events, integration.email_suppressions, \
                      integration.email_deliveries",
        )
        .execute(&owner_pool)
        .await
        .expect("clean notification fixture");
        sqlx::query("SELECT pgmq.purge_queue('chaos_email')")
            .execute(&owner_pool)
            .await
            .expect("clean email queue");
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();
        let store_a = Uuid::now_v7();
        let store_b = Uuid::now_v7();
        for (store_id, code) in [(store_a, "na"), (store_b, "nb")] {
            sqlx::query(
                "INSERT INTO commerce.stores (id, code, name) \
                 VALUES ($1, $2, 'Notification Store')",
            )
            .bind(store_id)
            .bind(format!("{code}-{suffix}"))
            .execute(&owner_pool)
            .await
            .expect("store");
        }
        let delivery_a = Uuid::now_v7();
        let delivery_b = Uuid::now_v7();
        let suppressed_delivery = Uuid::now_v7();
        for (id, store_id, recipient) in [
            (delivery_a, store_a, "a@example.com"),
            (delivery_b, store_b, "b@example.com"),
            (suppressed_delivery, store_a, "blocked@example.com"),
        ] {
            sqlx::query(
                "INSERT INTO integration.email_deliveries \
                 (id, store_id, semantic_event_id, semantic_event_type, \
                  recipient_email, template_key, template_version, template_payload) \
                 VALUES ($1, $2, $3, 'order.confirmed', $4, \
                         'order_confirmation', 1, $5)",
            )
            .bind(id)
            .bind(store_id)
            .bind(Uuid::now_v7())
            .bind(recipient)
            .bind(json!({
                "order_id": Uuid::now_v7(),
                "total_amount_minor": 1200,
                "currency": "USD"
            }))
            .execute(&owner_pool)
            .await
            .expect("delivery");
        }
        sqlx::query(
            "INSERT INTO integration.email_suppressions \
             (id, store_id, recipient_email, suppression_reason) \
             VALUES ($1, $2, 'blocked@example.com', 'manual')",
        )
        .bind(Uuid::now_v7())
        .bind(store_a)
        .execute(&owner_pool)
        .await
        .expect("suppression");
        let repository = PostgresEmailDeliveryRepository::new(runtime_pool.clone());
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let jobs = repository.claim(10).await.expect("claim");
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().any(|job| job.store_id == store_a));
        assert!(jobs.iter().any(|job| job.store_id == store_b));
        let suppressed_state: String = sqlx::query_scalar(
            "SELECT delivery_status::text FROM integration.email_deliveries WHERE id = $1",
        )
        .bind(suppressed_delivery)
        .fetch_one(&owner_pool)
        .await
        .expect("suppressed delivery state");
        assert_eq!(suppressed_state, "suppressed");
        repository
            .finish(
                delivery_a,
                1,
                Ok(EmailDelivery {
                    provider_message_id: "email_a".into(),
                }),
                now,
            )
            .await
            .expect("finish sent delivery");
        let delivery_b_job = jobs.iter().find(|job| job.id == delivery_b).unwrap();
        repository
            .finish(
                delivery_b,
                delivery_b_job.attempts,
                Err(EmailDeliveryFailure {
                    retryable: true,
                    message: "temporary outage".into(),
                }),
                now,
            )
            .await
            .expect("reschedule retry");
        sqlx::query(
            "SELECT pgmq.set_vt('chaos_email', pgmq_message_id, 0) \
             FROM integration.email_deliveries WHERE id = $1",
        )
        .bind(delivery_b)
        .execute(&owner_pool)
        .await
        .expect("make retry visible");
        let recovered = repository.claim(10).await.expect("claim retry");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, delivery_b);
        assert_eq!(recovered[0].attempts, 2);
        repository
            .finish(
                delivery_b,
                2,
                Ok(EmailDelivery {
                    provider_message_id: "email_b".into(),
                }),
                now + Duration::minutes(2),
            )
            .await
            .expect("reschedule retry");

        let delivered = VerifiedEmailWebhook {
            provider_event_id: "evt_delivered".into(),
            provider_message_id: "email_a".into(),
            provider_event_type: "email.delivered".into(),
            payload: json!({"type": "email.delivered", "data": {"email_id": "email_a"}}),
            received_at: now + Duration::minutes(3),
        };
        assert!(
            repository
                .record_webhook(&delivered)
                .await
                .expect("webhook")
        );
        assert!(
            !repository
                .record_webhook(&delivered)
                .await
                .expect("duplicate")
        );
        assert!(
            !repository
                .record_webhook(&VerifiedEmailWebhook {
                    provider_event_id: "evt_unknown".into(),
                    provider_message_id: "email_unknown".into(),
                    provider_event_type: "email.delivered".into(),
                    payload: json!({
                        "type": "email.delivered",
                        "data": {"email_id": "email_unknown"}
                    }),
                    received_at: now + Duration::minutes(3),
                })
                .await
                .expect("unknown delivery")
        );
        assert!(
            repository
                .record_webhook(&VerifiedEmailWebhook {
                    provider_event_id: "evt_complained".into(),
                    provider_message_id: "email_a".into(),
                    provider_event_type: "email.complained".into(),
                    payload: json!({
                        "type": "email.complained",
                        "data": {"email_id": "email_a"}
                    }),
                    received_at: now + Duration::minutes(4),
                })
                .await
                .expect("complaint")
        );
        let state: String = sqlx::query_scalar(
            "SELECT delivery_status::text FROM integration.email_deliveries WHERE id = $1",
        )
        .bind(delivery_a)
        .fetch_one(&owner_pool)
        .await
        .expect("delivery state");
        assert_eq!(state, "complained");

        let mut transaction = runtime_pool.begin().await.expect("RLS transaction");
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store_a.to_string())
            .execute(&mut *transaction)
            .await
            .expect("store context");
        let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM integration.email_deliveries")
            .fetch_one(&mut *transaction)
            .await
            .expect("visible deliveries");
        assert_eq!(visible, 2);
        transaction.rollback().await.expect("rollback");
    }
}
