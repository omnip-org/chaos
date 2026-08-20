use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, EmailDeliveryFailure, EmailDeliveryJob, EmailDeliveryRepository,
        IdempotencyRequest, NotificationProviderAccountConfiguration,
        NotificationProviderAccountDetail, NotificationProviderAccountRepository,
        ResolvedNotificationWebhook, VerifiedEmailWebhook,
    },
};
use chaos_domain::{
    notifications::{
        NotificationProviderAccount, NotificationProviderAccountId, NotificationSecretReference,
    },
    store::StoreId,
};
use serde_json::Value;
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CONFIGURE_PROVIDER_OPERATION: &str = "notification_provider_accounts.configure.v1";

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
        sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                String,
                String,
                i32,
                Value,
                String,
                Uuid,
                String,
                String,
                i32,
            ),
        >(
            "SELECT id, store_id, recipient_email, template_key, \
                    template_version, template_payload, provider, provider_account_id, \
                    credential_secret_reference, sender, attempts \
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
                provider_account_id: NotificationProviderAccountId::from_uuid(row.7),
                credential_secret_reference: NotificationSecretReference::new(row.8)?,
                sender: row.9,
                attempts: u32::try_from(row.10)
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

    async fn record_webhook(
        &self,
        provider_account_id: NotificationProviderAccountId,
        event: &VerifiedEmailWebhook,
    ) -> Result<bool, ApplicationError> {
        sqlx::query_scalar::<_, bool>(
            "SELECT integration.record_resend_webhook($1, $2, $3, $4, $5, $6)",
        )
        .bind(provider_account_id.as_uuid())
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

#[async_trait]
impl NotificationProviderAccountRepository for PostgresEmailDeliveryRepository {
    async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<NotificationProviderAccountDetail>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                bool,
                bool,
                bool,
                OffsetDateTime,
                OffsetDateTime,
            ),
        >(
            "SELECT id, provider, display_name, sender, enabled, \
                    credential_secret_reference IS NOT NULL, webhook_secret_reference IS NOT NULL, \
                    created_at, updated_at \
             FROM commerce.notification_provider_accounts \
             WHERE store_id = $1 ORDER BY created_at, id",
        )
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter().map(notification_detail).collect()
    }

    async fn configure(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        account: &NotificationProviderAccount,
        configuration: &NotificationProviderAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<NotificationProviderAccountDetail, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let replay = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::Store(store_id.as_uuid()),
            CONFIGURE_PROVIDER_OPERATION,
            request,
        )
        .await?;
        let id = if let Some(body) = replay {
            body.pointer("/data/id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(corrupt_provider_state)?
        } else {
            let id = sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO commerce.notification_provider_accounts \
                 (id, store_id, provider, display_name, sender, credential_secret_reference, \
                  webhook_secret_reference, enabled, created_by_user_id) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
                 ON CONFLICT (store_id, provider) DO UPDATE SET \
                    display_name = EXCLUDED.display_name, sender = EXCLUDED.sender, \
                    credential_secret_reference = EXCLUDED.credential_secret_reference, \
                    webhook_secret_reference = EXCLUDED.webhook_secret_reference, \
                    enabled = EXCLUDED.enabled, updated_at = CURRENT_TIMESTAMP \
                 RETURNING id",
            )
            .bind(account.id().as_uuid())
            .bind(store_id.as_uuid())
            .bind(account.provider())
            .bind(account.display_name())
            .bind(account.sender())
            .bind(configuration.credential_secret_reference.expose_reference())
            .bind(configuration.webhook_secret_reference.expose_reference())
            .bind(account.enabled())
            .bind(actor.audit_user_id().as_uuid())
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            idempotency::complete(
                &mut transaction,
                &IdempotencyScope::Store(store_id.as_uuid()),
                CONFIGURE_PROVIDER_OPERATION,
                request,
                200,
                json!({ "data": { "id": id } }),
            )
            .await?;
            id
        };
        let row = load_notification_account(&mut transaction, store_id, id)
            .await?
            .ok_or_else(corrupt_provider_state)?;
        transaction.commit().await.map_err(database_error)?;
        notification_detail(row)
    }

    async fn resolve_webhook(
        &self,
        account_id: NotificationProviderAccountId,
    ) -> Result<Option<ResolvedNotificationWebhook>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT store_id, provider, webhook_secret_reference \
             FROM commerce.resolve_notification_webhook($1)",
        )
        .bind(account_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?
        .map(|row| {
            Ok(ResolvedNotificationWebhook {
                store_id: StoreId::from_uuid(row.0),
                provider: row.1,
                webhook_secret_reference: NotificationSecretReference::new(row.2)?,
            })
        })
        .transpose()
    }
}

type NotificationAccountRow = (
    Uuid,
    String,
    String,
    String,
    bool,
    bool,
    bool,
    OffsetDateTime,
    OffsetDateTime,
);

impl PostgresEmailDeliveryRepository {
    async fn begin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(actor.store_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

async fn load_notification_account(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    id: Uuid,
) -> Result<Option<NotificationAccountRow>, ApplicationError> {
    sqlx::query_as(
        "SELECT id, provider, display_name, sender, enabled, \
                credential_secret_reference IS NOT NULL, webhook_secret_reference IS NOT NULL, \
                created_at, updated_at \
         FROM commerce.notification_provider_accounts WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)
}

fn notification_detail(
    row: NotificationAccountRow,
) -> Result<NotificationProviderAccountDetail, ApplicationError> {
    Ok(NotificationProviderAccountDetail {
        account: NotificationProviderAccount::rehydrate(
            NotificationProviderAccountId::from_uuid(row.0),
            row.1,
            row.2,
            row.3,
            row.4,
        )?,
        credentials_configured: row.5,
        webhook_configured: row.6,
        created_at: row.7,
        updated_at: row.8,
    })
}

fn corrupt_provider_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Notification Provider state"
    ))
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
        let owner_id = Uuid::now_v7();
        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_id)
            .bind(format!("notifications-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .expect("owner");
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
        let provider_a = Uuid::now_v7();
        let provider_b = Uuid::now_v7();
        for (id, store_id, sender) in [
            (provider_a, store_a, "a@example.com"),
            (provider_b, store_b, "b@example.com"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.notification_provider_accounts \
                 (id, store_id, provider, display_name, sender, credential_secret_reference, \
                  webhook_secret_reference, enabled, created_by_user_id) \
                 VALUES ($1, $2, 'resend', 'Primary', $3, 'enc://credential', \
                         'enc://webhook', true, $4)",
            )
            .bind(id)
            .bind(store_id)
            .bind(sender)
            .bind(owner_id)
            .execute(&owner_pool)
            .await
            .expect("notification provider");
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
            !repository
                .record_webhook(
                    NotificationProviderAccountId::from_uuid(provider_b),
                    &delivered,
                )
                .await
                .expect("cross-Store webhook")
        );
        assert!(
            repository
                .record_webhook(
                    NotificationProviderAccountId::from_uuid(provider_a),
                    &delivered
                )
                .await
                .expect("webhook")
        );
        assert!(
            !repository
                .record_webhook(
                    NotificationProviderAccountId::from_uuid(provider_a),
                    &delivered
                )
                .await
                .expect("duplicate")
        );
        assert!(
            !repository
                .record_webhook(
                    NotificationProviderAccountId::from_uuid(provider_a),
                    &VerifiedEmailWebhook {
                        provider_event_id: "evt_unknown".into(),
                        provider_message_id: "email_unknown".into(),
                        provider_event_type: "email.delivered".into(),
                        payload: json!({
                            "type": "email.delivered",
                            "data": {"email_id": "email_unknown"}
                        }),
                        received_at: now + Duration::minutes(3),
                    }
                )
                .await
                .expect("unknown delivery")
        );
        assert!(
            repository
                .record_webhook(
                    NotificationProviderAccountId::from_uuid(provider_a),
                    &VerifiedEmailWebhook {
                        provider_event_id: "evt_complained".into(),
                        provider_message_id: "email_a".into(),
                        provider_event_type: "email.complained".into(),
                        payload: json!({
                            "type": "email.complained",
                            "data": {"email_id": "email_a"}
                        }),
                        received_at: now + Duration::minutes(4),
                    }
                )
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
