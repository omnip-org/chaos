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
    async fn claim(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<EmailDeliveryJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, String, i32, Value, String, i32)>(
            "SELECT id, merchant_account_id, store_id, recipient_email, template_key, \
                    template_version, template_payload, provider, attempts \
             FROM notification.claim_email_deliveries($1, $2, $3, $4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit.clamp(1, 100)))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            Ok(EmailDeliveryJob {
                id: row.0,
                merchant_account_id: row.1,
                store_id: row.2,
                recipient_email: row.3,
                template_key: row.4,
                template_version: u32::try_from(row.5)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
                template_payload: row.6,
                provider: row.7,
                attempts: u32::try_from(row.8)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
            })
        })
        .collect()
    }

    async fn finish(
        &self,
        worker_id: Uuid,
        delivery_id: Uuid,
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
            "SELECT notification.finish_email_delivery($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(delivery_id)
        .bind(worker_id)
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
                code: "email_delivery_lease_lost",
                message: "The email delivery lease is no longer owned by this worker",
            })
        }
    }

    async fn record_webhook(&self, event: &VerifiedEmailWebhook) -> Result<bool, ApplicationError> {
        sqlx::query_scalar::<_, bool>(
            "SELECT notification.record_resend_webhook($1, $2, $3, $4, $5)",
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
    async fn delivery_leases_webhooks_and_suppressions_are_recoverable_and_isolated() {
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
            "TRUNCATE notification.webhook_events, notification.email_suppressions, \
                      notification.email_deliveries",
        )
        .execute(&owner_pool)
        .await
        .expect("clean notification fixture");
        let suffix = Uuid::now_v7().simple().to_string();
        let account_a = Uuid::now_v7();
        let account_b = Uuid::now_v7();
        let store_a = Uuid::now_v7();
        let store_b = Uuid::now_v7();
        for (account_id, slug) in [
            (account_a, format!("notify-a-{suffix}")),
            (account_b, format!("notify-b-{suffix}")),
        ] {
            sqlx::query(
                "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
                 VALUES ($1, $2, 'Notification Test')",
            )
            .bind(account_id)
            .bind(slug)
            .execute(&owner_pool)
            .await
            .expect("merchant account");
        }
        for (store_id, account_id, code) in [(store_a, account_a, "na"), (store_b, account_b, "nb")]
        {
            sqlx::query(
                "INSERT INTO merchant.stores (id, merchant_account_id, code, name) \
                 VALUES ($1, $2, $3, 'Notification Store')",
            )
            .bind(store_id)
            .bind(account_id)
            .bind(code)
            .execute(&owner_pool)
            .await
            .expect("store");
        }
        let delivery_a = Uuid::now_v7();
        let delivery_b = Uuid::now_v7();
        let suppressed_delivery = Uuid::now_v7();
        for (id, account_id, store_id, recipient) in [
            (delivery_a, account_a, store_a, "a@example.com"),
            (delivery_b, account_b, store_b, "b@example.com"),
            (
                suppressed_delivery,
                account_a,
                store_a,
                "blocked@example.com",
            ),
        ] {
            sqlx::query(
                "INSERT INTO notification.email_deliveries \
                 (id, merchant_account_id, store_id, semantic_event_id, semantic_event_type, \
                  recipient_email, template_key, template_version, template_payload) \
                 VALUES ($1, $2, $3, $4, 'order.confirmed', $5, \
                         'order_confirmation', 1, $6)",
            )
            .bind(id)
            .bind(account_id)
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
            "INSERT INTO notification.email_suppressions \
             (id, merchant_account_id, store_id, recipient_email, suppression_reason) \
             VALUES ($1, $2, $3, 'blocked@example.com', 'manual')",
        )
        .bind(Uuid::now_v7())
        .bind(account_a)
        .bind(store_a)
        .execute(&owner_pool)
        .await
        .expect("suppression");
        let dead_delivery = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO notification.email_deliveries \
             (id, merchant_account_id, store_id, semantic_event_id, semantic_event_type, \
              recipient_email, template_key, template_version, template_payload, \
              delivery_status, attempts, locked_by, locked_at) \
             VALUES ($1, $2, $3, $4, 'order.confirmed', 'dead@example.com', \
                     'order_confirmation', 1, $5, 'processing', 8, $6, \
                     CURRENT_TIMESTAMP - INTERVAL '10 minutes')",
        )
        .bind(dead_delivery)
        .bind(account_a)
        .bind(store_a)
        .bind(Uuid::now_v7())
        .bind(json!({
            "order_id": Uuid::now_v7(),
            "total_amount_minor": 1200,
            "currency": "USD"
        }))
        .bind(Uuid::now_v7())
        .execute(&owner_pool)
        .await
        .expect("exhausted delivery");

        let repository = PostgresEmailDeliveryRepository::new(runtime_pool.clone());
        let worker_a = Uuid::now_v7();
        // Keep the claim boundary strictly after database-generated availability timestamps.
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let jobs = repository
            .claim(worker_a, 10, now, now - Duration::minutes(1))
            .await
            .expect("claim");
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().any(|job| job.merchant_account_id == account_a));
        assert!(jobs.iter().any(|job| job.merchant_account_id == account_b));
        let dead_state: String = sqlx::query_scalar(
            "SELECT delivery_status::text FROM notification.email_deliveries WHERE id = $1",
        )
        .bind(dead_delivery)
        .fetch_one(&owner_pool)
        .await
        .expect("dead delivery state");
        assert_eq!(dead_state, "dead_letter");
        repository
            .finish(
                worker_a,
                delivery_a,
                Ok(EmailDelivery {
                    provider_message_id: "email_a".into(),
                }),
                now,
            )
            .await
            .expect("finish sent delivery");

        let worker_b = Uuid::now_v7();
        let recovered = repository
            .claim(
                worker_b,
                10,
                now + Duration::minutes(2),
                now + Duration::minutes(1),
            )
            .await
            .expect("recover stale lease");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, delivery_b);
        assert_eq!(recovered[0].attempts, 2);
        assert!(
            repository
                .finish(
                    worker_a,
                    delivery_b,
                    Err(EmailDeliveryFailure {
                        retryable: true,
                        message: "stale owner".into(),
                    }),
                    now + Duration::minutes(2),
                )
                .await
                .is_err()
        );
        repository
            .finish(
                worker_b,
                delivery_b,
                Err(EmailDeliveryFailure {
                    retryable: true,
                    message: "temporary outage".into(),
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
            "SELECT delivery_status::text FROM notification.email_deliveries WHERE id = $1",
        )
        .bind(delivery_a)
        .fetch_one(&owner_pool)
        .await
        .expect("delivery state");
        assert_eq!(state, "complained");

        let mut transaction = runtime_pool.begin().await.expect("RLS transaction");
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(account_a.to_string())
            .execute(&mut *transaction)
            .await
            .expect("account context");
        let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM notification.email_deliveries")
            .fetch_one(&mut *transaction)
            .await
            .expect("visible deliveries");
        assert_eq!(visible, 3);
        transaction.rollback().await.expect("rollback");
    }
}
