use std::{collections::HashMap, sync::Arc};

use chaos_domain::{
    payments::{
        PaymentAttemptId, PaymentProviderAccount, PaymentProviderAccountId, PaymentSecretReference,
    },
    sales::OrderId,
    store::{StoreId, StoreRole},
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const WORKER_LEASE_TIMEOUT: Duration = Duration::minutes(1);

use crate::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, IntegrationQueue, MAX_INTEGRATION_ATTEMPTS, MachineActor,
        PaymentAttemptDetail, PaymentClientAction, PaymentProvider,
        PaymentProviderAccountConfiguration, PaymentProviderAccountDetail,
        PaymentProviderAccountPage, PaymentProviderAccountRepository, PaymentProviderOnboarding,
        PaymentProviderReadinessQueue, PaymentRepository, PaymentWebhookVerifier, QueueJob,
        RefundDetail, ShopperActor,
    },
    store::StoreActor,
};

pub struct CreatePaymentAttemptInput {
    pub actor: ShopperActor,
    pub order_id: OrderId,
    pub provider: String,
    pub return_url: Option<String>,
    pub idempotency: IdempotencyRequest,
}

pub struct CreateRefundInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub payment_attempt_id: PaymentAttemptId,
    pub amount_minor: i64,
    pub idempotency: IdempotencyRequest,
}

pub struct CreatePaymentProviderAccountInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub provider: String,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
    pub enabled: bool,
    pub checked_at: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct UpdatePaymentProviderAccountInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub id: PaymentProviderAccountId,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
    pub enabled: bool,
    pub checked_at: OffsetDateTime,
    pub idempotency: IdempotencyRequest,
}

pub struct PaymentProviderAdministration {
    repository: Arc<dyn PaymentProviderAccountRepository>,
    onboarding: HashMap<String, Arc<dyn PaymentProviderOnboarding>>,
}

impl PaymentProviderAdministration {
    pub fn new(
        repository: Arc<dyn PaymentProviderAccountRepository>,
        onboarding: impl IntoIterator<Item = Arc<dyn PaymentProviderOnboarding>>,
    ) -> Self {
        Self {
            repository,
            onboarding: onboarding
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
        }
    }

    pub async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<PaymentProviderAccountPage, ApplicationError> {
        self.repository.list(actor, store_id, after, limit).await
    }

    pub async fn get(
        &self,
        actor: StoreActor,
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
        if !self.onboarding.contains_key(input.provider.as_str()) {
            return Err(unsupported_provider());
        }
        let mut account =
            PaymentProviderAccount::create(input.provider, input.display_name, input.enabled)?;
        let credential = PaymentSecretReference::new(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook = PaymentSecretReference::new(
            "webhook_secret_reference",
            input.webhook_secret_reference,
        )?;
        let configuration = self
            .configuration(&account, credential, webhook, input.checked_at)
            .await?;
        if configuration
            .readiness
            .as_ref()
            .is_some_and(|readiness| !readiness.ready)
        {
            account.update_administration(account.display_name().to_owned(), false)?;
        }
        self.repository
            .create(
                input.actor,
                input.store_id,
                &account,
                &configuration,
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
        if !self.onboarding.contains_key(detail.account.provider()) {
            return Err(unsupported_provider());
        }
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
        let configuration = self
            .configuration(&detail.account, credential, webhook, input.checked_at)
            .await?;
        if configuration
            .readiness
            .as_ref()
            .is_some_and(|readiness| !readiness.ready)
        {
            detail
                .account
                .update_administration(detail.account.display_name().to_owned(), false)?;
        }
        self.repository
            .update(
                input.actor,
                input.store_id,
                &detail.account,
                &configuration,
                &input.idempotency,
            )
            .await
    }

    async fn configuration(
        &self,
        account: &PaymentProviderAccount,
        credential_secret_reference: PaymentSecretReference,
        webhook_secret_reference: PaymentSecretReference,
        checked_at: OffsetDateTime,
    ) -> Result<PaymentProviderAccountConfiguration, ApplicationError> {
        let readiness = if account.enabled() {
            let provider =
                self.onboarding
                    .get(account.provider())
                    .ok_or(ApplicationError::Conflict {
                        code: "payment_provider_not_supported",
                        message: "The Payment Provider cannot be enabled by this deployment",
                    })?;
            let readiness = provider
                .check_readiness(&credential_secret_reference, checked_at)
                .await?;
            Some(readiness)
        } else {
            None
        };
        Ok(PaymentProviderAccountConfiguration {
            credential_secret_reference,
            webhook_secret_reference,
            readiness,
        })
    }
}

pub struct PaymentService {
    repository: Arc<dyn PaymentRepository>,
    verifiers: HashMap<String, Arc<dyn PaymentWebhookVerifier>>,
    providers: HashMap<String, Arc<dyn PaymentProvider>>,
}

impl PaymentService {
    pub fn new(
        repository: Arc<dyn PaymentRepository>,
        verifiers: impl IntoIterator<Item = Arc<dyn PaymentWebhookVerifier>>,
        providers: impl IntoIterator<Item = Arc<dyn PaymentProvider>>,
    ) -> Self {
        Self {
            repository,
            verifiers: verifiers
                .into_iter()
                .map(|verifier| (verifier.name().to_owned(), verifier))
                .collect(),
            providers: providers
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
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
                input.return_url.as_deref(),
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

    pub async fn get_client_action(
        &self,
        actor: &ShopperActor,
        attempt_id: PaymentAttemptId,
    ) -> Result<PaymentClientAction, ApplicationError> {
        require_checkout_key(&actor.machine)?;
        self.repository
            .get_attempt(actor, attempt_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "payment_attempt",
                id: attempt_id.as_uuid().to_string(),
            })?;
        let (provider_name, command) = self
            .repository
            .client_action_command(actor, attempt_id)
            .await?
            .ok_or(ApplicationError::Conflict {
                code: "payment_client_action_not_ready",
                message: "the Payment Attempt client action is not available",
            })?;
        let provider = self
            .providers
            .get(&provider_name)
            .ok_or(ApplicationError::Unavailable {
                service: "payment_provider",
                source: anyhow::anyhow!("provider adapter is not configured"),
            })?;
        provider.client_action(command).await
    }

    pub async fn create_refund(
        &self,
        input: CreateRefundInput,
    ) -> Result<RefundDetail, ApplicationError> {
        require_payment_operator(&input.actor)?;
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
        provider_account_id: Uuid,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<bool, ApplicationError> {
        let verifier = self
            .verifiers
            .get(provider)
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "payment_provider",
                id: provider.to_owned(),
            })?;
        let event = verifier
            .verify(
                provider,
                provider_account_id,
                signature,
                payload,
                received_at,
            )
            .await?;
        self.repository.ingest_webhook(&event).await
    }
}

pub struct PaymentWorkers {
    queue: Arc<dyn IntegrationQueue>,
    readiness_queue: Arc<dyn PaymentProviderReadinessQueue>,
    repository: Arc<dyn PaymentRepository>,
    providers: HashMap<String, Arc<dyn PaymentProvider>>,
    onboarding: HashMap<String, Arc<dyn PaymentProviderOnboarding>>,
}

impl PaymentWorkers {
    pub fn new(
        queue: Arc<dyn IntegrationQueue>,
        readiness_queue: Arc<dyn PaymentProviderReadinessQueue>,
        repository: Arc<dyn PaymentRepository>,
        providers: impl IntoIterator<Item = Arc<dyn PaymentProvider>>,
        onboarding: impl IntoIterator<Item = Arc<dyn PaymentProviderOnboarding>>,
    ) -> Self {
        Self {
            queue,
            readiness_queue,
            repository,
            providers: providers
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
            onboarding: onboarding
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
        }
    }

    pub async fn run_outbox_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self.queue.claim_outbox(limit).await?;
        for job in &jobs {
            let result = self
                .execute_provider_job(job, now)
                .await
                .map_err(|error| error.to_string());
            if job.event_type == "payment.create_requested"
                && result.is_err()
                && job.attempts >= MAX_INTEGRATION_ATTEMPTS
            {
                let failure = result.as_ref().expect_err("result is an error");
                self.repository
                    .fail_provider_command(job, failure, now)
                    .await?;
            }
            self.queue
                .finish_outbox(job.id, job.attempts, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    pub async fn run_webhook_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self.queue.claim_webhooks(limit).await?;
        for job in &jobs {
            let result = self
                .repository
                .process_webhook_job(job, now)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_webhook(job.id, job.attempts, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    pub async fn run_readiness_batch(
        &self,
        worker_id: Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .readiness_queue
            .claim_provider_readiness(worker_id, limit, now, now - WORKER_LEASE_TIMEOUT)
            .await?;
        for job in &jobs {
            let result = match self.onboarding.get(&job.provider) {
                Some(provider) => provider
                    .check_readiness(&job.credential_secret_reference, now)
                    .await
                    .map_err(|error| error.to_string()),
                None => Err("Provider onboarding adapter is not configured".into()),
            };
            self.readiness_queue
                .finish_provider_readiness(worker_id, job.provider_account_id, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    async fn execute_provider_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
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
        let command = self.repository.prepare_provider_command(job).await?;
        let result = provider.execute(command).await?;
        self.repository
            .record_provider_result(job, &result, now)
            .await?;
        Ok(())
    }
}

pub fn retry_at(now: OffsetDateTime, attempts: u32) -> OffsetDateTime {
    let exponent = attempts.saturating_sub(1).min(8);
    now + Duration::seconds(2_i64.pow(exponent))
}

fn require_checkout_key(actor: &MachineActor) -> Result<(), ApplicationError> {
    if actor.sales_channel_id.is_some() {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn require_payment_operator(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}

fn require_provider_administrator(actor: StoreActor) -> Result<(), ApplicationError> {
    if actor.role() == StoreRole::Owner {
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

fn unsupported_provider() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_not_supported",
        message: "Only stripe_checkout is supported by this deployment",
    }
}

fn invalid_outbox_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("invalid payment outbox payload"))
}
