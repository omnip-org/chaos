use std::{collections::HashMap, sync::Arc};

use chaos_domain::{
    merchant::{ApiKeyClass, ApiKeyScope, MerchantRole, StoreId},
    payments::{
        PaymentAttemptId, PaymentProviderAccount, PaymentProviderAccountId, PaymentSecretReference,
    },
    sales::OrderId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const WORKER_LEASE_TIMEOUT: Duration = Duration::minutes(1);

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        IdempotencyRequest, IntegrationQueue, MachineActor, PaymentAttemptDetail, PaymentProvider,
        PaymentProviderAccountDetail, PaymentProviderAccountPage, PaymentProviderAccountRepository,
        PaymentRepository, PaymentWebhookVerifier, ProviderCommand, QueueJob, RefundDetail,
        ShopperActor,
    },
};

pub struct CreatePaymentAttemptInput {
    pub actor: ShopperActor,
    pub order_id: OrderId,
    pub provider: String,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateRefundInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub payment_attempt_id: PaymentAttemptId,
    pub amount_minor: i64,
    pub idempotency: IdempotencyRequest,
}

pub struct CreatePaymentProviderAccountInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub provider: String,
    pub display_name: String,
    pub external_account_reference: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
    pub idempotency: IdempotencyRequest,
}

pub struct UpdatePaymentProviderAccountInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub id: PaymentProviderAccountId,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
    pub enabled: bool,
    pub idempotency: IdempotencyRequest,
}

pub struct PaymentProviderAdministration {
    repository: Arc<dyn PaymentProviderAccountRepository>,
}

impl PaymentProviderAdministration {
    pub fn new(repository: Arc<dyn PaymentProviderAccountRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<PaymentProviderAccountPage, ApplicationError> {
        self.repository.list(actor, store_id, after, limit).await
    }

    pub async fn get(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        id: PaymentProviderAccountId,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError> {
        self.repository
            .get(actor, store_id, id)
            .await?
            .ok_or_else(|| provider_account_not_found(id))
    }

    pub async fn create(
        &self,
        input: CreatePaymentProviderAccountInput,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError> {
        require_provider_administrator(input.actor)?;
        let account = PaymentProviderAccount::create(
            input.provider,
            input.display_name,
            input.external_account_reference,
        )?;
        let credential = PaymentSecretReference::new(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook = PaymentSecretReference::new(
            "webhook_secret_reference",
            input.webhook_secret_reference,
        )?;
        self.repository
            .create(
                input.actor,
                input.store_id,
                &account,
                &credential,
                &webhook,
                &input.idempotency,
            )
            .await
    }

    pub async fn update(
        &self,
        input: UpdatePaymentProviderAccountInput,
    ) -> Result<PaymentProviderAccountDetail, ApplicationError> {
        require_provider_administrator(input.actor)?;
        let mut detail = self
            .repository
            .get(input.actor, input.store_id, input.id)
            .await?
            .ok_or_else(|| provider_account_not_found(input.id))?;
        detail
            .account
            .update_administration(input.display_name, input.enabled)?;
        let credential = PaymentSecretReference::new(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook = PaymentSecretReference::new(
            "webhook_secret_reference",
            input.webhook_secret_reference,
        )?;
        self.repository
            .update(
                input.actor,
                input.store_id,
                &detail.account,
                &credential,
                &webhook,
                &input.idempotency,
            )
            .await
    }
}

pub struct PaymentService {
    repository: Arc<dyn PaymentRepository>,
    verifier: Arc<dyn PaymentWebhookVerifier>,
}

impl PaymentService {
    pub fn new(
        repository: Arc<dyn PaymentRepository>,
        verifier: Arc<dyn PaymentWebhookVerifier>,
    ) -> Self {
        Self {
            repository,
            verifier,
        }
    }

    pub async fn create_attempt(
        &self,
        input: CreatePaymentAttemptInput,
    ) -> Result<PaymentAttemptDetail, ApplicationError> {
        require_checkout_key(&input.actor.machine)?;
        self.repository
            .create_attempt(
                &input.actor,
                input.order_id,
                &input.provider,
                &input.idempotency,
            )
            .await
    }

    pub async fn get_attempt(
        &self,
        actor: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<PaymentAttemptDetail, ApplicationError> {
        require_checkout_key(&actor.machine)?;
        self.repository
            .get_attempt(actor, attempt_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "payment_attempt",
                id: attempt_id.as_uuid().to_string(),
            })
    }

    pub async fn create_refund(
        &self,
        input: CreateRefundInput,
    ) -> Result<RefundDetail, ApplicationError> {
        require_payment_operator(input.actor)?;
        self.repository
            .create_refund(
                input.actor,
                input.store_id,
                input.payment_attempt_id,
                input.amount_minor,
                &input.idempotency,
            )
            .await
    }

    pub async fn receive_webhook(
        &self,
        provider: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<bool, ApplicationError> {
        let event = self
            .verifier
            .verify(provider, signature, payload, received_at)?;
        self.repository.ingest_webhook(&event).await
    }
}

pub struct PaymentWorkers {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<dyn PaymentRepository>,
    providers: HashMap<String, Arc<dyn PaymentProvider>>,
}

impl PaymentWorkers {
    pub fn new(
        queue: Arc<dyn IntegrationQueue>,
        repository: Arc<dyn PaymentRepository>,
        providers: impl IntoIterator<Item = Arc<dyn PaymentProvider>>,
    ) -> Self {
        Self {
            queue,
            repository,
            providers: providers
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
        }
    }

    pub async fn run_outbox_batch(
        &self,
        worker_id: Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_outbox(worker_id, limit, now, now - WORKER_LEASE_TIMEOUT)
            .await?;
        for job in &jobs {
            let result = self
                .execute_provider_job(job)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_outbox(worker_id, job.id, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    pub async fn run_webhook_batch(
        &self,
        worker_id: Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_webhooks(worker_id, limit, now, now - WORKER_LEASE_TIMEOUT)
            .await?;
        for job in &jobs {
            let result = self
                .repository
                .process_webhook_job(job, now)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_webhook(worker_id, job.id, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    async fn execute_provider_job(&self, job: &QueueJob) -> Result<(), ApplicationError> {
        let provider_name = job
            .payload
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid_outbox_payload)?;
        let provider = self
            .providers
            .get(provider_name)
            .ok_or(ApplicationError::Unavailable {
                service: "payment_provider",
                source: anyhow::anyhow!("provider adapter is not configured"),
            })?;
        let aggregate_id = job
            .payload
            .get("aggregate_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(invalid_outbox_payload)?;
        let amount_minor = job
            .payload
            .get("amount_minor")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(invalid_outbox_payload)?;
        let currency_text = job
            .payload
            .get("currency")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid_outbox_payload)?;
        let currency = chaos_domain::CurrencyCode::parse(currency_text)?;
        provider
            .execute(ProviderCommand {
                event_type: job.event_type.clone(),
                aggregate_id,
                amount_minor,
                currency,
                idempotency_key: job.id.to_string(),
            })
            .await?;
        Ok(())
    }
}

pub fn retry_at(now: OffsetDateTime, attempts: u32) -> OffsetDateTime {
    let exponent = attempts.saturating_sub(1).min(8);
    now + Duration::seconds(2_i64.pow(exponent))
}

fn require_checkout_key(actor: &MachineActor) -> Result<(), ApplicationError> {
    if actor.class == ApiKeyClass::Publishable
        && actor.sales_channel_id.is_some()
        && actor.scopes.contains(&ApiKeyScope::CheckoutWrite)
    {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn require_payment_operator(actor: MerchantActor) -> Result<(), ApplicationError> {
    if matches!(
        actor.role(),
        MerchantRole::Owner | MerchantRole::Administrator | MerchantRole::Manager
    ) {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn require_provider_administrator(actor: MerchantActor) -> Result<(), ApplicationError> {
    if matches!(
        actor.role(),
        MerchantRole::Owner | MerchantRole::Administrator
    ) {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn provider_account_not_found(id: PaymentProviderAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "payment_provider_account",
        id: id.as_uuid().to_string(),
    }
}

fn invalid_outbox_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("invalid payment outbox payload"))
}
