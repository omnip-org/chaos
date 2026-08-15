use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        IdempotencyRequest, IntegrationQueue, MachineActor, PaymentAttemptDetail,
        PaymentClientAction, PaymentProvider, PaymentProviderAccountConfiguration,
        PaymentProviderAccountDetail, PaymentProviderAccountPage, PaymentProviderAccountRepository,
        PaymentProviderOnboarding, PaymentProviderReadiness, PaymentProviderReadinessJob,
        PaymentProviderReadinessQueue, PaymentProviderReadinessStatus, PaymentRepository,
        PaymentWebhookConfigurationRepository, PaymentWebhookVerifier, ProviderClientActionCommand,
        ProviderCommand, ProviderCommandResult, QueueJob, RefundDetail, ShopperActor,
        VerifiedWebhookEvent,
    },
};
use chaos_domain::{
    CurrencyCode,
    inventory::InventoryReservationId,
    merchant::{SalesChannelId, StoreId},
    payments::{
        PaymentAttempt, PaymentAttemptId, PaymentAttemptStatus, PaymentProviderAccount,
        PaymentProviderAccountId, PaymentSecretReference, Refund, RefundId, RefundStatus,
    },
    pricing::Money,
    sales::{CheckoutId, Order, OrderId, OrderStatus},
};
use hmac::{Hmac, Mac};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    idempotency::{self, IdempotencyScope},
    inventory::{ReservationClosure, close_reservation},
};

const CREATE_ATTEMPT_OPERATION: &str = "payment_attempts.create.v1";
const CREATE_REFUND_OPERATION: &str = "refunds.create.v1";
const CREATE_PROVIDER_ACCOUNT_OPERATION: &str = "payment_provider_accounts.create.v1";
const UPDATE_PROVIDER_ACCOUNT_OPERATION: &str = "payment_provider_accounts.update.v1";
const MAX_QUEUE_ATTEMPTS: i32 = 8;

type ProviderAccountRow = (
    Uuid,
    String,
    String,
    String,
    bool,
    bool,
    String,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    Value,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebhookPayload {
    id: String,
    event_type: String,
    account: String,
    object: String,
    #[serde(rename = "aggregate_id")]
    _aggregate_id: Uuid,
    #[serde(default)]
    failure_code: Option<String>,
}

pub struct HmacPaymentWebhookVerifier {
    secret: Vec<u8>,
}

pub struct SandboxPaymentProvider;

#[async_trait]
impl PaymentProvider for SandboxPaymentProvider {
    fn name(&self) -> &'static str {
        "testpay"
    }

    async fn execute(
        &self,
        command: ProviderCommand,
    ) -> Result<ProviderCommandResult, ApplicationError> {
        Ok(ProviderCommandResult {
            provider_reference: format!("testpay_{}", command.aggregate_id.simple()),
        })
    }

    async fn client_action(
        &self,
        command: ProviderClientActionCommand,
    ) -> Result<PaymentClientAction, ApplicationError> {
        Ok(PaymentClientAction {
            provider: "testpay".into(),
            kind: "confirm_payment",
            public_key: SecretString::from("testpay_public".to_owned()),
            client_token: SecretString::from(format!(
                "{}_client_token",
                command.provider_reference
            )),
            account_reference: command.external_account_reference,
        })
    }
}

#[async_trait]
impl PaymentProviderOnboarding for SandboxPaymentProvider {
    fn name(&self) -> &'static str {
        "testpay"
    }

    async fn check_readiness(
        &self,
        _external_account_reference: &str,
        _credential_secret_reference: &PaymentSecretReference,
        checked_at: OffsetDateTime,
    ) -> Result<PaymentProviderReadiness, ApplicationError> {
        Ok(PaymentProviderReadiness {
            ready: true,
            blocker_codes: Vec::new(),
            configuration: json!({"sandbox": true}),
            checked_at,
        })
    }
}

impl HmacPaymentWebhookVerifier {
    pub fn new(secret: impl AsRef<[u8]>) -> Result<Self, anyhow::Error> {
        let secret = secret.as_ref();
        if secret.len() < 32 {
            anyhow::bail!("payment webhook secret must contain at least 32 bytes");
        }
        Ok(Self {
            secret: secret.to_vec(),
        })
    }
}

#[async_trait]
impl PaymentWebhookVerifier for HmacPaymentWebhookVerifier {
    fn name(&self) -> &'static str {
        "testpay"
    }

    async fn verify(
        &self,
        provider: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedWebhookEvent, ApplicationError> {
        let signature = STANDARD
            .decode(signature)
            .map_err(|_| ApplicationError::Unauthorized)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        mac.update(payload);
        mac.verify_slice(&signature)
            .map_err(|_| ApplicationError::Unauthorized)?;
        if provider.trim().is_empty()
            || provider.len() > 64
            || !provider
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(ApplicationError::NotFound {
                resource: "payment_provider",
                id: provider.to_owned(),
            });
        }
        let raw: Value =
            serde_json::from_slice(payload).map_err(|_| ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "payload",
                    reason: "must be a valid payment webhook event".into(),
                }],
            })?;
        let event: WebhookPayload =
            serde_json::from_value(raw.clone()).map_err(|_| ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "payload",
                    reason: "must contain the required payment webhook fields".into(),
                }],
            })?;
        Ok(VerifiedWebhookEvent {
            provider: provider.to_owned(),
            provider_event_id: event.id,
            event_type: event.event_type,
            external_account_reference: event.account,
            object_reference: event.object,
            failure_code: event.failure_code,
            payload: raw,
            verified_at: received_at,
        })
    }
}

#[derive(Clone)]
pub struct PostgresPaymentRepository {
    pool: PgPool,
}

impl PostgresPaymentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_machine(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        self.begin_context(None, actor.merchant_account_id.as_uuid())
            .await
    }

    async fn begin_shopper(
        &self,
        shopper: &ShopperActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.begin_machine(&shopper.machine).await?;
        set_config(
            &mut transaction,
            "app.shopper_id",
            shopper.shopper_id.as_uuid(),
        )
        .await?;
        Ok(transaction)
    }

    async fn begin_human(
        &self,
        actor: MerchantActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        self.begin_context(
            Some(actor.user_id().as_uuid()),
            actor.merchant_account_id().as_uuid(),
        )
        .await
    }

    async fn begin_context(
        &self,
        user_id: Option<Uuid>,
        account_id: Uuid,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        if let Some(user_id) = user_id {
            set_config(&mut transaction, "app.user_id", user_id).await?;
        }
        set_config(&mut transaction, "app.merchant_account_id", account_id).await?;
        Ok(transaction)
    }
}

#[async_trait]
impl PaymentProviderAccountRepository for PostgresPaymentRepository {
    async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<PaymentProviderAccountPage, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let rows = sqlx::query_as::<_, ProviderAccountRow>(
            "SELECT id, provider, display_name, external_account_reference, enabled, \
                    credential_secret_reference IS NOT NULL AND webhook_secret_reference IS NOT NULL, \
                    readiness_status, readiness_checked_at, readiness_valid_until, \
                    COALESCE(readiness_snapshot->'blocker_codes', '[]'::jsonb), \
                    credential_rotation_expires_at, webhook_rotation_expires_at, \
                    created_at, updated_at \
             FROM payments.provider_accounts \
             WHERE merchant_account_id = $1 AND store_id = $2 \
               AND ($3::uuid IS NULL OR id < $3) \
             ORDER BY id DESC LIMIT $4",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let has_more = rows.len() > usize::from(limit);
        let items = rows
            .into_iter()
            .take(usize::from(limit))
            .map(provider_account_detail)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await.map_err(database_error)?;
        Ok(PaymentProviderAccountPage { items, has_more })
    }

    async fn get(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        id: PaymentProviderAccountId,
    ) -> Result<Option<PaymentProviderAccountDetail>, ApplicationError> {
        let mut transaction = self.begin_human(actor).await?;
        let value = load_provider_account(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            store_id,
            id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn create(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        account: &PaymentProviderAccount,
        configuration: &PaymentProviderAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin_human(actor).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::MerchantAccount(account_id),
            CREATE_PROVIDER_ACCOUNT_OPERATION,
            request,
        )
        .await?
        {
            return replay_provider_account(&mut transaction, account_id, store_id, snapshot).await;
        }
        let readiness = configuration.readiness.as_ref();
        sqlx::query(
            "INSERT INTO payments.provider_accounts \
             (id, merchant_account_id, store_id, provider, display_name, \
              external_account_reference, credential_secret_reference, webhook_secret_reference, \
              readiness_status, readiness_snapshot, readiness_checked_at, \
              readiness_valid_until, readiness_reconcile_at, \
              enabled, created_by_user_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
        )
        .bind(account.id().as_uuid())
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(account.provider())
        .bind(account.display_name())
        .bind(account.external_account_reference())
        .bind(configuration.credential_secret_reference.expose_reference())
        .bind(configuration.webhook_secret_reference.expose_reference())
        .bind(readiness.map_or(
            PaymentProviderReadinessStatus::Unchecked.as_str(),
            |value| readiness_status(value).as_str(),
        ))
        .bind(readiness.map(|value| &value.configuration))
        .bind(readiness.map(|value| value.checked_at))
        .bind(
            readiness
                .filter(|value| value.ready)
                .map(|value| value.checked_at + time::Duration::hours(24)),
        )
        .bind(
            readiness
                .filter(|value| value.ready)
                .map(|value| value.checked_at + time::Duration::hours(6)),
        )
        .bind(account.enabled())
        .bind(actor.user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        complete_provider_account(
            &mut transaction,
            account_id,
            CREATE_PROVIDER_ACCOUNT_OPERATION,
            request,
            account.id(),
        )
        .await?;
        let value = load_provider_account(&mut transaction, account_id, store_id, account.id())
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }

    async fn update(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        account: &PaymentProviderAccount,
        configuration: &PaymentProviderAccountConfiguration,
        request: &IdempotencyRequest,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin_human(actor).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::MerchantAccount(account_id),
            UPDATE_PROVIDER_ACCOUNT_OPERATION,
            request,
        )
        .await?
        {
            return replay_provider_account(&mut transaction, account_id, store_id, snapshot).await;
        }
        let readiness = configuration.readiness.as_ref();
        let result = sqlx::query(
            "UPDATE payments.provider_accounts SET display_name = $4, \
                    previous_credential_secret_reference = CASE \
                        WHEN credential_secret_reference IS NOT NULL \
                             AND credential_secret_reference IS DISTINCT FROM $5 \
                        THEN credential_secret_reference ELSE previous_credential_secret_reference END, \
                    credential_rotation_expires_at = CASE \
                        WHEN credential_secret_reference IS NOT NULL \
                             AND credential_secret_reference IS DISTINCT FROM $5 \
                        THEN CURRENT_TIMESTAMP + INTERVAL '24 hours' ELSE credential_rotation_expires_at END, \
                    credential_secret_reference = $5, \
                    previous_webhook_secret_reference = CASE \
                        WHEN webhook_secret_reference IS NOT NULL \
                             AND webhook_secret_reference IS DISTINCT FROM $6 \
                        THEN webhook_secret_reference ELSE previous_webhook_secret_reference END, \
                    webhook_rotation_expires_at = CASE \
                        WHEN webhook_secret_reference IS NOT NULL \
                             AND webhook_secret_reference IS DISTINCT FROM $6 \
                        THEN CURRENT_TIMESTAMP + INTERVAL '24 hours' ELSE webhook_rotation_expires_at END, \
                    webhook_secret_reference = $6, \
                    readiness_status = CASE \
                        WHEN $8::text IS NOT NULL THEN $8 \
                        WHEN credential_secret_reference IS DISTINCT FROM $5 THEN 'unchecked' \
                        ELSE readiness_status END, \
                    readiness_snapshot = CASE \
                        WHEN $8::text IS NOT NULL THEN $9::jsonb \
                        WHEN credential_secret_reference IS DISTINCT FROM $5 THEN NULL \
                        ELSE readiness_snapshot END, \
                    readiness_checked_at = CASE \
                        WHEN $8::text IS NOT NULL THEN $10::timestamptz \
                        WHEN credential_secret_reference IS DISTINCT FROM $5 THEN NULL \
                        ELSE readiness_checked_at END, \
                    readiness_valid_until = CASE \
                        WHEN $8::text = 'ready' THEN $10::timestamptz + INTERVAL '24 hours' \
                        WHEN $8::text IS NOT NULL THEN NULL \
                        WHEN credential_secret_reference IS DISTINCT FROM $5 THEN NULL \
                        ELSE readiness_valid_until END, \
                    readiness_reconcile_at = CASE \
                        WHEN $8::text = 'ready' THEN $10::timestamptz + INTERVAL '6 hours' \
                        WHEN $8::text IS NOT NULL THEN NULL \
                        WHEN credential_secret_reference IS DISTINCT FROM $5 THEN NULL \
                        ELSE readiness_reconcile_at END, \
                    readiness_locked_by = CASE \
                        WHEN $8::text IS NOT NULL OR credential_secret_reference IS DISTINCT FROM $5 \
                        THEN NULL ELSE readiness_locked_by END, \
                    readiness_locked_at = CASE \
                        WHEN $8::text IS NOT NULL OR credential_secret_reference IS DISTINCT FROM $5 \
                        THEN NULL ELSE readiness_locked_at END, \
                    readiness_reconcile_attempts = CASE \
                        WHEN $8::text IS NOT NULL OR credential_secret_reference IS DISTINCT FROM $5 \
                        THEN 0 ELSE readiness_reconcile_attempts END, \
                    readiness_last_error = CASE \
                        WHEN $8::text IS NOT NULL OR credential_secret_reference IS DISTINCT FROM $5 \
                        THEN NULL ELSE readiness_last_error END, \
                    enabled = $7, updated_at = CURRENT_TIMESTAMP \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(account.id().as_uuid())
        .bind(account.display_name())
        .bind(
            configuration
                .credential_secret_reference
                .expose_reference(),
        )
        .bind(configuration.webhook_secret_reference.expose_reference())
        .bind(account.enabled())
        .bind(readiness.map(|value| readiness_status(value).as_str()))
        .bind(readiness.map(|value| &value.configuration))
        .bind(readiness.map(|value| value.checked_at))
        .execute(&mut *transaction)
        .await
        .map_err(map_provider_account_write_error)?;
        if result.rows_affected() != 1 {
            return Err(provider_account_not_found(account.id()));
        }
        complete_provider_account(
            &mut transaction,
            account_id,
            UPDATE_PROVIDER_ACCOUNT_OPERATION,
            request,
            account.id(),
        )
        .await?;
        let value = load_provider_account(&mut transaction, account_id, store_id, account.id())
            .await?
            .ok_or_else(corrupt_state)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(value)
    }
}

#[async_trait]
impl PaymentWebhookConfigurationRepository for PostgresPaymentRepository {
    async fn webhook_secret_references(
        &self,
        provider: &str,
        external_account_reference: &str,
    ) -> Result<Vec<PaymentSecretReference>, ApplicationError> {
        sqlx::query_scalar::<_, String>(
            "SELECT secret_reference \
             FROM payments.resolve_provider_webhook_secret_references($1, $2)",
        )
        .bind(provider)
        .bind(external_account_reference)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|reference| {
            PaymentSecretReference::new("webhook_secret_reference", reference)
                .map_err(ApplicationError::from)
        })
        .collect()
    }
}

#[async_trait]
impl PaymentProviderReadinessQueue for PostgresPaymentRepository {
    async fn claim_provider_readiness(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<PaymentProviderReadinessJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, String, String, i32)>(
            "SELECT provider_account_id, merchant_account_id, store_id, provider, \
                    external_account_reference, credential_secret_reference, attempts \
             FROM payments.claim_provider_readiness_checks($1, $2, $3, $4)",
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
            Ok(PaymentProviderReadinessJob {
                provider_account_id: PaymentProviderAccountId::from_uuid(row.0),
                merchant_account_id: row.1,
                store_id: StoreId::from_uuid(row.2),
                provider: row.3,
                external_account_reference: row.4,
                credential_secret_reference: PaymentSecretReference::new(
                    "credential_secret_reference",
                    row.5,
                )?,
                attempts: u32::try_from(row.6)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
            })
        })
        .collect()
    }

    async fn finish_provider_readiness(
        &self,
        worker_id: Uuid,
        provider_account_id: PaymentProviderAccountId,
        result: Result<PaymentProviderReadiness, String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let (succeeded, ready, snapshot, checked_at, failure) = match result {
            Ok(readiness) => (
                true,
                readiness.ready,
                readiness.configuration,
                readiness.checked_at,
                String::new(),
            ),
            Err(failure) => (false, false, Value::Null, now, failure),
        };
        let finished: Option<bool> = sqlx::query_scalar(
            "SELECT payments.finish_provider_readiness_check($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(provider_account_id.as_uuid())
        .bind(worker_id)
        .bind(succeeded)
        .bind(ready)
        .bind(snapshot)
        .bind(checked_at)
        .bind(failure)
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?;
        if finished == Some(true) {
            Ok(())
        } else {
            Err(queue_job_not_found())
        }
    }
}

#[async_trait]
impl PaymentRepository for PostgresPaymentRepository {
    async fn create_attempt(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
        provider: &str,
        request: &IdempotencyRequest,
    ) -> Result<PaymentAttemptDetail, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let order = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT total_amount_minor, currency::text, status::text FROM sales.orders \
             WHERE merchant_account_id = $1 AND store_id = $2 AND sales_channel_id = $3 \
               AND id = $4 AND shopper_id = $5 FOR UPDATE",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        if order.2 != "pending" {
            return Err(payment_order_not_pending());
        }
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_ATTEMPT_OPERATION,
            request,
        )
        .await?
        {
            return replay_attempt(snapshot);
        }
        let provider_account_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM payments.provider_accounts \
             WHERE merchant_account_id = $1 AND store_id = $2 AND provider = $3 AND enabled \
               AND readiness_status = 'ready' AND readiness_valid_until > CURRENT_TIMESTAMP \
             ORDER BY id LIMIT 1",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(provider)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(provider_unavailable)?;
        let active_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM payments.payment_attempts \
             WHERE merchant_account_id = $1 AND store_id = $2 AND order_id = $3 \
               AND status IN ('pending', 'authorized', 'captured'))",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if active_exists {
            return Err(active_attempt_exists());
        }
        let currency = CurrencyCode::parse(&order.1)?;
        let attempt = PaymentAttempt::create(order_id, Money::new(order.0, currency))?;
        sqlx::query(
            "INSERT INTO payments.payment_attempts \
             (id, merchant_account_id, store_id, order_id, shopper_id, provider_account_id, \
              amount_minor, currency) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(attempt.id().as_uuid())
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(provider_account_id)
        .bind(attempt.amount().amount_minor())
        .bind(currency.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        insert_outbox(
            &mut transaction,
            actor.merchant_account_id.as_uuid(),
            actor.store_id,
            "payment_attempt",
            attempt.id().as_uuid(),
            "payment.create_requested",
            provider,
            attempt.amount().amount_minor(),
            currency,
        )
        .await?;
        let detail = load_attempt(
            &mut transaction,
            actor.merchant_account_id.as_uuid(),
            actor.store_id,
            Some(channel_id),
            Some(shopper.shopper_id.as_uuid()),
            attempt.id(),
        )
        .await?
        .ok_or_else(|| attempt_not_found(attempt.id()))?;
        idempotency::complete(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_ATTEMPT_OPERATION,
            request,
            201,
            attempt_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn get_attempt(
        &self,
        shopper: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<Option<PaymentAttemptDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        let detail = load_attempt(
            &mut transaction,
            actor.merchant_account_id.as_uuid(),
            actor.store_id,
            actor.sales_channel_id,
            Some(shopper.shopper_id.as_uuid()),
            attempt_id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_refund(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        attempt_id: PaymentAttemptId,
        amount_minor: i64,
        request: &IdempotencyRequest,
    ) -> Result<RefundDetail, ApplicationError> {
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin_human(actor).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::MerchantAccount(account_id),
            CREATE_REFUND_OPERATION,
            request,
        )
        .await?
        {
            return replay_refund(snapshot);
        }
        let row = sqlx::query_as::<_, (Uuid, i64, String, String, Option<String>, String)>(
            "SELECT attempt.order_id, attempt.amount_minor, attempt.currency::text, \
                    attempt.status::text, attempt.provider_reference, account.provider \
             FROM payments.payment_attempts AS attempt \
             INNER JOIN payments.provider_accounts AS account \
               ON account.merchant_account_id = attempt.merchant_account_id \
              AND account.store_id = attempt.store_id AND account.id = attempt.provider_account_id \
             WHERE attempt.merchant_account_id = $1 AND attempt.store_id = $2 \
               AND attempt.id = $3 FOR UPDATE OF attempt",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| attempt_not_found(attempt_id))?;
        let currency = CurrencyCode::parse(&row.2)?;
        let attempt = PaymentAttempt::rehydrate(
            attempt_id,
            OrderId::from_uuid(row.0),
            Money::new(row.1, currency),
            PaymentAttemptStatus::parse(&row.3).ok_or_else(corrupt_payment_state)?,
            row.4,
        );
        let already_refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM payments.refunds \
             WHERE merchant_account_id = $1 AND store_id = $2 AND payment_attempt_id = $3 \
               AND status IN ('pending', 'succeeded')",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let refund = Refund::create(
            &attempt,
            Money::new(amount_minor, currency),
            already_refunded,
        )?;
        sqlx::query(
            "INSERT INTO payments.refunds \
             (id, merchant_account_id, store_id, payment_attempt_id, amount_minor, currency) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(refund.id().as_uuid())
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(amount_minor)
        .bind(currency.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let id = refund.id();
        insert_outbox(
            &mut transaction,
            account_id,
            store_id,
            "refund",
            id.as_uuid(),
            "refund.create_requested",
            &row.5,
            amount_minor,
            currency,
        )
        .await?;
        let detail = load_refund(&mut transaction, account_id, store_id, id)
            .await?
            .ok_or_else(|| refund_not_found(id))?;
        idempotency::complete(
            &mut transaction,
            &IdempotencyScope::MerchantAccount(account_id),
            CREATE_REFUND_OPERATION,
            request,
            201,
            refund_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn ingest_webhook(&self, event: &VerifiedWebhookEvent) -> Result<bool, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let account = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
            "SELECT provider_account_id, merchant_account_id, store_id \
             FROM payments.resolve_provider_account($1, $2)",
        )
        .bind(&event.provider)
        .bind(&event.external_account_reference)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(provider_unavailable)?;
        set_config(&mut transaction, "app.merchant_account_id", account.1).await?;
        let result = sqlx::query(
            "INSERT INTO integration.webhook_inbox \
             (id, merchant_account_id, store_id, provider, provider_event_id, event_type, \
              external_account_reference, payload, verified_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (provider, provider_event_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(account.1)
        .bind(account.2)
        .bind(&event.provider)
        .bind(&event.provider_event_id)
        .bind(&event.event_type)
        .bind(&event.external_account_reference)
        .bind(&event.payload)
        .bind(event.verified_at)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn process_webhook_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin_context(None, job.merchant_account_id).await?;
        let aggregate_id = job
            .payload
            .get("aggregate_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(corrupt_webhook_payload)?;
        let provider_reference = job
            .payload
            .get("object")
            .and_then(Value::as_str)
            .ok_or_else(corrupt_webhook_payload)?
            .to_owned();
        let failure_code = job
            .payload
            .get("failure_code")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if job.event_type.starts_with("payment.") {
            apply_payment_event(
                &mut transaction,
                job.merchant_account_id,
                StoreId::from_uuid(job.store_id),
                PaymentAttemptId::from_uuid(aggregate_id),
                &job.event_type,
                provider_reference,
                failure_code,
                now,
            )
            .await?;
        } else if job.event_type.starts_with("refund.") {
            apply_refund_event(
                &mut transaction,
                job.merchant_account_id,
                StoreId::from_uuid(job.store_id),
                RefundId::from_uuid(aggregate_id),
                &job.event_type,
                provider_reference,
                failure_code,
                now,
            )
            .await?;
        } else {
            return Err(corrupt_webhook_payload());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    async fn prepare_provider_command(
        &self,
        job: &QueueJob,
    ) -> Result<ProviderCommand, ApplicationError> {
        let aggregate_id = outbox_aggregate_id(job)?;
        let provider = outbox_provider(job)?;
        let mut transaction = self.begin_context(None, job.merchant_account_id).await?;
        let row = if job.event_type == "payment.create_requested" {
            sqlx::query_as::<_, (i64, String, String, String, Option<String>)>(
                "SELECT attempt.amount_minor, attempt.currency::text, \
                        account.external_account_reference, account.credential_secret_reference, \
                        NULL::text \
                 FROM payments.payment_attempts AS attempt \
                 INNER JOIN payments.provider_accounts AS account \
                   ON account.merchant_account_id = attempt.merchant_account_id \
                  AND account.store_id = attempt.store_id \
                  AND account.id = attempt.provider_account_id \
                 WHERE attempt.merchant_account_id = $1 AND attempt.store_id = $2 \
                   AND attempt.id = $3 AND account.provider = $4 \
                   AND account.credential_secret_reference IS NOT NULL",
            )
            .bind(job.merchant_account_id)
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(provider)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
        } else if job.event_type == "refund.create_requested" {
            sqlx::query_as::<_, (i64, String, String, String, Option<String>)>(
                "SELECT refund.amount_minor, refund.currency::text, \
                        account.external_account_reference, account.credential_secret_reference, \
                        attempt.provider_reference \
                 FROM payments.refunds AS refund \
                 INNER JOIN payments.payment_attempts AS attempt \
                   ON attempt.merchant_account_id = refund.merchant_account_id \
                  AND attempt.store_id = refund.store_id \
                  AND attempt.id = refund.payment_attempt_id \
                 INNER JOIN payments.provider_accounts AS account \
                   ON account.merchant_account_id = attempt.merchant_account_id \
                  AND account.store_id = attempt.store_id \
                  AND account.id = attempt.provider_account_id \
                 WHERE refund.merchant_account_id = $1 AND refund.store_id = $2 \
                   AND refund.id = $3 AND account.provider = $4 \
                   AND account.credential_secret_reference IS NOT NULL",
            )
            .bind(job.merchant_account_id)
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(provider)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
        } else {
            return Err(invalid_outbox_payload());
        }
        .ok_or_else(provider_unavailable)?;
        transaction.commit().await.map_err(database_error)?;
        if row.0 != outbox_amount(job)? || row.1 != outbox_currency(job)? {
            return Err(invalid_outbox_payload());
        }
        if job.event_type == "refund.create_requested" && row.4.is_none() {
            return Err(ApplicationError::Conflict {
                code: "payment_provider_reference_missing",
                message: "the captured Payment Attempt has no provider reference",
            });
        }
        Ok(ProviderCommand {
            event_type: job.event_type.clone(),
            aggregate_id,
            amount_minor: row.0,
            currency: CurrencyCode::parse(&row.1)?,
            idempotency_key: job.id.to_string(),
            external_account_reference: row.2,
            credential_secret_reference: PaymentSecretReference::new(
                "credential_secret_reference",
                row.3,
            )?,
            payment_provider_reference: row.4,
        })
    }

    async fn record_provider_result(
        &self,
        job: &QueueJob,
        result: &ProviderCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if result.provider_reference.trim().is_empty()
            || result.provider_reference.chars().count() > 255
        {
            return Err(provider_invalid_response());
        }
        let aggregate_id = outbox_aggregate_id(job)?;
        let mut transaction = self.begin_context(None, job.merchant_account_id).await?;
        let rows = if job.event_type == "payment.create_requested" {
            sqlx::query(
                "UPDATE payments.payment_attempts \
                 SET provider_reference = COALESCE(provider_reference, $4), \
                     updated_at = CASE WHEN provider_reference IS NULL THEN $5 ELSE updated_at END \
                 WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
                   AND (provider_reference IS NULL OR provider_reference = $4)",
            )
            .bind(job.merchant_account_id)
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(&result.provider_reference)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
            .rows_affected()
        } else if job.event_type == "refund.create_requested" {
            sqlx::query(
                "UPDATE payments.refunds \
                 SET provider_reference = COALESCE(provider_reference, $4), \
                     updated_at = CASE WHEN provider_reference IS NULL THEN $5 ELSE updated_at END \
                 WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
                   AND (provider_reference IS NULL OR provider_reference = $4)",
            )
            .bind(job.merchant_account_id)
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(&result.provider_reference)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
            .rows_affected()
        } else {
            return Err(invalid_outbox_payload());
        };
        if rows != 1 {
            return Err(provider_reference_mismatch());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    async fn client_action_command(
        &self,
        shopper: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<Option<(String, ProviderClientActionCommand)>, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let row = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT account.provider, attempt.provider_reference, \
                    account.external_account_reference, account.credential_secret_reference \
             FROM payments.payment_attempts AS attempt \
             INNER JOIN payments.provider_accounts AS account \
               ON account.merchant_account_id = attempt.merchant_account_id \
              AND account.store_id = attempt.store_id AND account.id = attempt.provider_account_id \
             INNER JOIN sales.orders AS sales_order \
               ON sales_order.merchant_account_id = attempt.merchant_account_id \
              AND sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
             WHERE attempt.merchant_account_id = $1 AND attempt.store_id = $2 \
               AND attempt.id = $3 AND attempt.shopper_id = $4 \
               AND sales_order.sales_channel_id = $5 AND attempt.status IN ('pending', 'authorized') \
               AND attempt.provider_reference IS NOT NULL \
               AND account.credential_secret_reference IS NOT NULL",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(channel_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        row.map(|row| {
            Ok((
                row.0,
                ProviderClientActionCommand {
                    provider_reference: row.1,
                    external_account_reference: row.2,
                    credential_secret_reference: PaymentSecretReference::new(
                        "credential_secret_reference",
                        row.3,
                    )?,
                },
            ))
        })
        .transpose()
    }
}

#[async_trait]
impl IntegrationQueue for PostgresPaymentRepository {
    async fn claim_outbox(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<QueueJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, Value, i32)>(
            "SELECT id, merchant_account_id, store_id, event_type, payload, attempts \
             FROM integration.claim_outbox_events($1, $2, $3, $4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit.clamp(1, 100)))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(queue_job)
        .collect()
    }

    async fn claim_webhooks(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<QueueJob>, ApplicationError> {
        sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, String, Value, i32)>(
            "SELECT id, merchant_account_id, store_id, provider, event_type, payload, attempts \
             FROM integration.claim_webhook_events($1, $2, $3, $4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit.clamp(1, 100)))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| queue_job((row.0, row.1, row.2, row.4, row.5, row.6)))
        .collect()
    }

    async fn finish_outbox(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        finish_job(&self.pool, false, worker_id, job_id, result, now).await
    }

    async fn finish_webhook(
        &self,
        worker_id: Uuid,
        job_id: Uuid,
        result: Result<(), String>,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        finish_job(&self.pool, true, worker_id, job_id, result, now).await
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_outbox(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    aggregate_type: &'static str,
    aggregate_id: Uuid,
    event_type: &'static str,
    provider: &str,
    amount_minor: i64,
    currency: CurrencyCode,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO integration.outbox_events \
         (id, merchant_account_id, store_id, aggregate_type, aggregate_id, event_type, payload) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(aggregate_type)
    .bind(aggregate_id)
    .bind(event_type)
    .bind(json!({
        "provider": provider,
        "aggregate_id": aggregate_id,
        "amount_minor": amount_minor,
        "currency": currency.as_str(),
    }))
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn load_attempt(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    channel_id: Option<SalesChannelId>,
    shopper_id: Option<Uuid>,
    attempt_id: PaymentAttemptId,
) -> Result<Option<PaymentAttemptDetail>, ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            String,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            OffsetDateTime,
            OffsetDateTime,
        ),
    >(
        "SELECT attempt.id, attempt.order_id, account.provider, attempt.amount_minor, \
                attempt.currency::text, attempt.status::text, attempt.provider_reference, \
                attempt.failure_code, attempt.created_at, attempt.updated_at \
         FROM payments.payment_attempts AS attempt \
         INNER JOIN payments.provider_accounts AS account \
           ON account.merchant_account_id = attempt.merchant_account_id \
          AND account.store_id = attempt.store_id AND account.id = attempt.provider_account_id \
         INNER JOIN sales.orders AS sales_order \
           ON sales_order.merchant_account_id = attempt.merchant_account_id \
          AND sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
         WHERE attempt.merchant_account_id = $1 AND attempt.store_id = $2 AND attempt.id = $3 \
           AND ($4::uuid IS NULL OR sales_order.sales_channel_id = $4) \
           AND ($5::uuid IS NULL OR attempt.shopper_id = $5)",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .bind(channel_id.map(SalesChannelId::as_uuid))
    .bind(shopper_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        Ok(PaymentAttemptDetail {
            id: PaymentAttemptId::from_uuid(row.0),
            order_id: OrderId::from_uuid(row.1),
            provider: row.2,
            amount_minor: row.3,
            currency: CurrencyCode::parse(&row.4)?,
            status: PaymentAttemptStatus::parse(&row.5).ok_or_else(corrupt_payment_state)?,
            provider_reference: row.6,
            failure_code: row.7,
            created_at: row.8,
            updated_at: row.9,
        })
    })
    .transpose()
}

async fn load_refund(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    refund_id: RefundId,
) -> Result<Option<RefundDetail>, ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            OffsetDateTime,
            OffsetDateTime,
        ),
    >(
        "SELECT id, payment_attempt_id, amount_minor, currency::text, status::text, \
                provider_reference, failure_code, created_at, updated_at \
         FROM payments.refunds WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(refund_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        Ok(RefundDetail {
            id: RefundId::from_uuid(row.0),
            payment_attempt_id: PaymentAttemptId::from_uuid(row.1),
            amount_minor: row.2,
            currency: CurrencyCode::parse(&row.3)?,
            status: RefundStatus::parse(&row.4).ok_or_else(corrupt_payment_state)?,
            provider_reference: row.5,
            failure_code: row.6,
            created_at: row.7,
            updated_at: row.8,
        })
    })
    .transpose()
}

#[allow(clippy::too_many_arguments)]
async fn apply_payment_event(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    attempt_id: PaymentAttemptId,
    event_type: &str,
    provider_reference: String,
    failure_code: Option<String>,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            i64,
            String,
            String,
            Option<String>,
            Option<Uuid>,
            String,
            Uuid,
        ),
    >(
        "SELECT attempt.order_id, attempt.amount_minor, attempt.currency::text, \
                attempt.status::text, attempt.provider_reference, \
                sales_order.inventory_reservation_id, sales_order.status::text, \
                sales_order.checkout_id \
         FROM payments.payment_attempts AS attempt \
         INNER JOIN sales.orders AS sales_order \
           ON sales_order.merchant_account_id = attempt.merchant_account_id \
          AND sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
         WHERE attempt.merchant_account_id = $1 AND attempt.store_id = $2 AND attempt.id = $3 \
         FOR UPDATE OF attempt, sales_order",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| attempt_not_found(attempt_id))?;
    let currency = CurrencyCode::parse(&row.2)?;
    let mut attempt = PaymentAttempt::rehydrate(
        attempt_id,
        OrderId::from_uuid(row.0),
        Money::new(row.1, currency),
        PaymentAttemptStatus::parse(&row.3).ok_or_else(corrupt_payment_state)?,
        row.4,
    );
    let changed = match event_type {
        "payment.authorized" => attempt.authorize(provider_reference)?,
        "payment.captured" => {
            if attempt.status() == PaymentAttemptStatus::Pending {
                attempt.authorize(provider_reference)?;
            } else if attempt.provider_reference() != Some(provider_reference.as_str()) {
                return Err(provider_reference_mismatch());
            }
            attempt.capture()?
        }
        "payment.failed" => attempt.fail(Some(provider_reference))?,
        "payment.cancelled" => attempt.cancel(Some(provider_reference))?,
        _ => return Err(corrupt_webhook_payload()),
    };
    if !changed {
        return Ok(());
    }
    let stored_failure = if attempt.status() == PaymentAttemptStatus::Failed {
        Some(failure_code.unwrap_or_else(|| "provider_failure".into()))
    } else {
        None
    };
    sqlx::query(
        "UPDATE payments.payment_attempts \
         SET status = $4::payments.payment_attempt_status, provider_reference = $5, \
             failure_code = $6, updated_at = $7 \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(attempt_id.as_uuid())
    .bind(attempt.status().as_str())
    .bind(attempt.provider_reference())
    .bind(stored_failure)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if attempt.status() == PaymentAttemptStatus::Captured {
        confirm_paid_order(
            transaction,
            account_id,
            store_id,
            OrderId::from_uuid(row.0),
            CheckoutId::from_uuid(row.7),
            row.5.map(InventoryReservationId::from_uuid),
            &row.6,
            now,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn confirm_paid_order(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
    checkout_id: CheckoutId,
    reservation_id: Option<InventoryReservationId>,
    status: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let status = OrderStatus::parse(status).ok_or_else(corrupt_payment_state)?;
    if status == OrderStatus::Confirmed {
        return Ok(());
    }
    let mut order = Order::rehydrate(order_id, checkout_id, status);
    let transition = order.confirm(now)?;
    if let Some(reservation_id) = reservation_id {
        close_reservation(
            transaction,
            account_id,
            store_id,
            reservation_id,
            ReservationClosure::Consumed,
            now,
        )
        .await?;
    }
    sqlx::query(
        "UPDATE sales.orders SET status = 'confirmed', updated_at = $4 \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    let transition_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sales.order_transitions \
         (id, merchant_account_id, store_id, order_id, from_status, to_status, kind, occurred_at) \
         VALUES ($1, $2, $3, $4, $5::sales.order_status, 'confirmed', 'confirmed', $6)",
    )
    .bind(transition_id)
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(transition.from_status.map(OrderStatus::as_str))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO notification.email_deliveries \
         (id, merchant_account_id, store_id, semantic_event_id, semantic_event_type, \
          recipient_email, template_key, template_version, template_payload, provider) \
         SELECT $1, order_row.merchant_account_id, order_row.store_id, $2, \
                'order.confirmed', contact.email, 'order_confirmation', 1, \
                jsonb_build_object( \
                    'order_id', order_row.id, \
                    'total_amount_minor', order_row.total_amount_minor, \
                    'currency', order_row.currency::text \
                ), 'resend' \
           FROM sales.orders AS order_row \
           INNER JOIN sales.order_contacts AS contact \
             ON contact.merchant_account_id = order_row.merchant_account_id \
            AND contact.store_id = order_row.store_id \
            AND contact.order_id = order_row.id \
          WHERE order_row.merchant_account_id = $3 \
            AND order_row.store_id = $4 AND order_row.id = $5 \
         ON CONFLICT (merchant_account_id, store_id, semantic_event_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(transition_id)
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_refund_event(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    refund_id: RefundId,
    event_type: &str,
    provider_reference: String,
    failure_code: Option<String>,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, i64, String, String, Option<String>)>(
        "SELECT payment_attempt_id, amount_minor, currency::text, status::text, provider_reference \
         FROM payments.refunds WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
         FOR UPDATE",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(refund_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| refund_not_found(refund_id))?;
    let mut refund = Refund::rehydrate(
        refund_id,
        PaymentAttemptId::from_uuid(row.0),
        Money::new(row.1, CurrencyCode::parse(&row.2)?),
        RefundStatus::parse(&row.3).ok_or_else(corrupt_payment_state)?,
        row.4,
    );
    let changed = match event_type {
        "refund.succeeded" => refund.succeed(provider_reference)?,
        "refund.failed" => refund.fail(provider_reference)?,
        _ => return Err(corrupt_webhook_payload()),
    };
    if !changed {
        return Ok(());
    }
    let stored_failure = if refund.status() == RefundStatus::Failed {
        Some(failure_code.unwrap_or_else(|| "provider_failure".into()))
    } else {
        None
    };
    sqlx::query(
        "UPDATE payments.refunds \
         SET status = $4::payments.refund_status, provider_reference = $5, \
             failure_code = $6, updated_at = $7 \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(refund_id.as_uuid())
    .bind(refund.status().as_str())
    .bind(refund.provider_reference())
    .bind(stored_failure)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn finish_job(
    pool: &PgPool,
    webhook: bool,
    worker_id: Uuid,
    job_id: Uuid,
    result: Result<(), String>,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let (succeeded, failure) = match result {
        Ok(()) => (true, String::new()),
        Err(failure) => (false, failure),
    };
    let finished: Option<bool> = if webhook {
        sqlx::query_scalar("SELECT integration.finish_webhook_event($1, $2, $3, $4, $5, $6)")
            .bind(job_id)
            .bind(worker_id)
            .bind(succeeded)
            .bind(&failure)
            .bind(MAX_QUEUE_ATTEMPTS)
            .bind(now)
            .fetch_one(pool)
            .await
            .map_err(database_error)?
    } else {
        sqlx::query_scalar("SELECT integration.finish_outbox_event($1, $2, $3, $4, $5, $6)")
            .bind(job_id)
            .bind(worker_id)
            .bind(succeeded)
            .bind(&failure)
            .bind(MAX_QUEUE_ATTEMPTS)
            .bind(now)
            .fetch_one(pool)
            .await
            .map_err(database_error)?
    };
    if finished == Some(true) {
        Ok(())
    } else {
        Err(queue_job_not_found())
    }
}

fn queue_job(row: (Uuid, Uuid, Uuid, String, Value, i32)) -> Result<QueueJob, ApplicationError> {
    Ok(QueueJob {
        id: row.0,
        merchant_account_id: row.1,
        store_id: row.2,
        event_type: row.3,
        payload: row.4,
        attempts: u32::try_from(row.5)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
    })
}

#[derive(Serialize, Deserialize)]
struct AttemptSnapshot {
    id: Uuid,
    order_id: Uuid,
    provider: String,
    amount_minor: i64,
    currency: String,
    status: String,
    provider_reference: Option<String>,
    failure_code: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct RefundSnapshot {
    id: Uuid,
    payment_attempt_id: Uuid,
    amount_minor: i64,
    currency: String,
    status: String,
    provider_reference: Option<String>,
    failure_code: Option<String>,
    created_at: String,
    updated_at: String,
}

fn attempt_snapshot(detail: &PaymentAttemptDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(AttemptSnapshot {
        id: detail.id.as_uuid(),
        order_id: detail.order_id.as_uuid(),
        provider: detail.provider.clone(),
        amount_minor: detail.amount_minor,
        currency: detail.currency.as_str().into(),
        status: detail.status.as_str().into(),
        provider_reference: detail.provider_reference.clone(),
        failure_code: detail.failure_code.clone(),
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_attempt(value: Value) -> Result<PaymentAttemptDetail, ApplicationError> {
    let value: AttemptSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(PaymentAttemptDetail {
        id: PaymentAttemptId::from_uuid(value.id),
        order_id: OrderId::from_uuid(value.order_id),
        provider: value.provider,
        amount_minor: value.amount_minor,
        currency: CurrencyCode::parse(&value.currency)?,
        status: PaymentAttemptStatus::parse(&value.status).ok_or_else(corrupt_payment_state)?,
        provider_reference: value.provider_reference,
        failure_code: value.failure_code,
        created_at: parse_time(&value.created_at)?,
        updated_at: parse_time(&value.updated_at)?,
    })
}

fn refund_snapshot(detail: &RefundDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(RefundSnapshot {
        id: detail.id.as_uuid(),
        payment_attempt_id: detail.payment_attempt_id.as_uuid(),
        amount_minor: detail.amount_minor,
        currency: detail.currency.as_str().into(),
        status: detail.status.as_str().into(),
        provider_reference: detail.provider_reference.clone(),
        failure_code: detail.failure_code.clone(),
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_refund(value: Value) -> Result<RefundDetail, ApplicationError> {
    let value: RefundSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(RefundDetail {
        id: RefundId::from_uuid(value.id),
        payment_attempt_id: PaymentAttemptId::from_uuid(value.payment_attempt_id),
        amount_minor: value.amount_minor,
        currency: CurrencyCode::parse(&value.currency)?,
        status: RefundStatus::parse(&value.status).ok_or_else(corrupt_payment_state)?,
        provider_reference: value.provider_reference,
        failure_code: value.failure_code,
        created_at: parse_time(&value.created_at)?,
        updated_at: parse_time(&value.updated_at)?,
    })
}

async fn set_config(
    transaction: &mut Transaction<'static, Postgres>,
    key: &'static str,
    value: Uuid,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT set_config($1, $2, true)")
        .bind(key)
        .bind(value.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(())
}

fn format_time(value: OffsetDateTime) -> Result<String, ApplicationError> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn parse_time(value: &str) -> Result<OffsetDateTime, ApplicationError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn invalid_snapshot(error: serde_json::Error) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "invalid payment idempotency snapshot: {error}"
    ))
}

fn outbox_aggregate_id(job: &QueueJob) -> Result<Uuid, ApplicationError> {
    job.payload
        .get("aggregate_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_provider(job: &QueueJob) -> Result<&str, ApplicationError> {
    job.payload
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_amount(job: &QueueJob) -> Result<i64, ApplicationError> {
    job.payload
        .get("amount_minor")
        .and_then(Value::as_i64)
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_currency(job: &QueueJob) -> Result<&str, ApplicationError> {
    job.payload
        .get("currency")
        .and_then(Value::as_str)
        .ok_or_else(invalid_outbox_payload)
}

fn invalid_outbox_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("payment outbox payload is invalid"))
}

fn provider_invalid_response() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "payment_provider",
        source: anyhow::anyhow!("provider returned an invalid reference"),
    }
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn attempt_not_found(attempt_id: PaymentAttemptId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "payment_attempt",
        id: attempt_id.as_uuid().to_string(),
    }
}

fn refund_not_found(refund_id: RefundId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "refund",
        id: refund_id.as_uuid().to_string(),
    }
}

async fn load_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    id: PaymentProviderAccountId,
) -> Result<Option<PaymentProviderAccountDetail>, ApplicationError> {
    sqlx::query_as::<_, ProviderAccountRow>(
        "SELECT id, provider, display_name, external_account_reference, enabled, \
                credential_secret_reference IS NOT NULL AND webhook_secret_reference IS NOT NULL, \
                readiness_status, readiness_checked_at, readiness_valid_until, \
                COALESCE(readiness_snapshot->'blocker_codes', '[]'::jsonb), \
                credential_rotation_expires_at, webhook_rotation_expires_at, \
                created_at, updated_at FROM payments.provider_accounts \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(provider_account_detail)
    .transpose()
}

fn provider_account_detail(
    row: ProviderAccountRow,
) -> Result<PaymentProviderAccountDetail, ApplicationError> {
    Ok(PaymentProviderAccountDetail {
        account: PaymentProviderAccount::rehydrate(
            PaymentProviderAccountId::from_uuid(row.0),
            row.1,
            row.2,
            row.3,
            row.4,
        )?,
        credentials_configured: row.5,
        readiness_status: match row.6.as_str() {
            "unchecked" => PaymentProviderReadinessStatus::Unchecked,
            "ready" => PaymentProviderReadinessStatus::Ready,
            "action_required" => PaymentProviderReadinessStatus::ActionRequired,
            _ => return Err(corrupt_state()),
        },
        readiness_checked_at: row.7,
        readiness_valid_until: row.8,
        readiness_blocker_codes: serde_json::from_value(row.9).map_err(|_| corrupt_state())?,
        credential_rotation_expires_at: row.10,
        webhook_rotation_expires_at: row.11,
        created_at: row.12,
        updated_at: row.13,
    })
}

fn readiness_status(readiness: &PaymentProviderReadiness) -> PaymentProviderReadinessStatus {
    if readiness.ready {
        PaymentProviderReadinessStatus::Ready
    } else {
        PaymentProviderReadinessStatus::ActionRequired
    }
}

async fn replay_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    snapshot: Value,
) -> Result<PaymentProviderAccountDetail, ApplicationError> {
    let id = snapshot
        .pointer("/data/id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(PaymentProviderAccountId::from_uuid)
        .ok_or_else(corrupt_state)?;
    load_provider_account(transaction, account_id, store_id, id)
        .await?
        .ok_or_else(corrupt_state)
}

async fn complete_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: PaymentProviderAccountId,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::MerchantAccount(account_id),
        operation,
        request,
        200,
        json!({"data":{"id":id.as_uuid()}}),
    )
    .await
}

fn map_provider_account_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error {
        let (code, message) = match database.constraint() {
            Some("provider_accounts_store_provider_key") => (
                "payment_provider_already_configured",
                "the Payment Provider is already configured for this Store",
            ),
            Some("provider_accounts_provider_external_account_reference_key") => (
                "payment_provider_account_already_linked",
                "the external Payment Provider account is already linked",
            ),
            _ => return database_error(error),
        };
        return ApplicationError::Conflict { code, message };
    }
    database_error(error)
}

fn provider_account_not_found(id: PaymentProviderAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "payment_provider_account",
        id: id.as_uuid().to_string(),
    }
}

fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Payment Provider account state"
    ))
}

fn provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_unavailable",
        message: "no enabled Payment Provider account is available",
    }
}

fn payment_order_not_pending() -> ApplicationError {
    ApplicationError::Conflict {
        code: "order_not_pending_payment",
        message: "the Order is not awaiting payment",
    }
}

fn active_attempt_exists() -> ApplicationError {
    ApplicationError::Conflict {
        code: "active_payment_attempt_exists",
        message: "the Order already has an active Payment Attempt",
    }
}

fn provider_reference_mismatch() -> ApplicationError {
    ApplicationError::Conflict {
        code: "provider_reference_mismatch",
        message: "the provider reference does not match the Payment Attempt",
    }
}

fn corrupt_payment_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains an unknown Payment state"
    ))
}

fn corrupt_webhook_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("verified webhook payload is invalid"))
}

fn queue_job_not_found() -> ApplicationError {
    ApplicationError::Conflict {
        code: "queue_lease_lost",
        message: "the queue job is no longer leased by this worker",
    }
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            ApplicationError::Unavailable {
                service: "postgresql",
                source: error.into(),
            }
        }
        _ => ApplicationError::Unexpected(error.into()),
    }
}
