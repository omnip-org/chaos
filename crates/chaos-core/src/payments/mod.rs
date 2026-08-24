use std::sync::Arc;

use chaos_domain::{
    sales::OrderId,
    store::{StoreId, StoreRole},
    stripe::{PaymentSecretReference, StripeAccount, StripeAccountId},
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::PostgresStripeRepository,
    contracts::{
        AdminActor, IntegrationQueue, MachineActor, PaymentAttemptDetail, PaymentClientAction,
        QueueJob, RefundDetail, ShopperActor, StripeAccountConfiguration, StripeAccountDetail,
        StripeAccountPage, StripePaymentGateway, StripeWebhookSignatureVerifier,
    },
    store::StoreActor,
};

pub struct CreatePaymentAttemptInput {
    pub actor: ShopperActor,
    pub order_id: OrderId,
    pub return_url: Option<String>,
    pub now: OffsetDateTime,
    pub request_id: uuid::Uuid,
}

pub struct EmbeddedCheckoutResult {
    pub attempt: PaymentAttemptDetail,
    pub client_action: PaymentClientAction,
}

pub struct CreateRefundInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub amount_minor: i64,
}

pub struct CreateStripeAccountInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
}

pub struct UpdateStripeAccountInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub id: StripeAccountId,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
}

pub struct StripeAccountAdministration {
    repository: Arc<PostgresStripeRepository>,
}

impl StripeAccountAdministration {
    pub fn new(repository: Arc<PostgresStripeRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<StripeAccountPage, ApplicationError> {
        self.repository.list(actor, store_id, after, limit).await
    }

    pub async fn get(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        id: StripeAccountId,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        self.repository
            .get(actor, store_id, id)
            .await?
            .ok_or_else(|| stripe_account_not_found(id))
    }

    pub async fn create(
        &self,
        input: CreateStripeAccountInput,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        require_stripe_account_administrator(input.actor)?;
        let account = StripeAccount::create(input.display_name)?;
        let credential = PaymentSecretReference::new(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook = PaymentSecretReference::new(
            "webhook_secret_reference",
            input.webhook_secret_reference,
        )?;
        let configuration = StripeAccountConfiguration {
            credential_secret_reference: credential,
            webhook_secret_reference: webhook,
        };
        self.repository
            .create(input.actor, input.store_id, &account, &configuration)
            .await
    }

    pub async fn update(
        &self,
        input: UpdateStripeAccountInput,
    ) -> Result<StripeAccountDetail, ApplicationError> {
        require_stripe_account_administrator(input.actor)?;
        let mut detail = self
            .repository
            .get(input.actor, input.store_id, input.id)
            .await?
            .ok_or_else(|| stripe_account_not_found(input.id))?;
        detail.account.update_administration(input.display_name)?;
        let credential = PaymentSecretReference::new(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook = PaymentSecretReference::new(
            "webhook_secret_reference",
            input.webhook_secret_reference,
        )?;
        let configuration = StripeAccountConfiguration {
            credential_secret_reference: credential,
            webhook_secret_reference: webhook,
        };
        self.repository
            .update(input.actor, input.store_id, &detail.account, &configuration)
            .await
    }
}

pub struct PaymentService {
    repository: Arc<PostgresStripeRepository>,
    verifier: Arc<dyn StripeWebhookSignatureVerifier>,
    stripe_gateway: Arc<dyn StripePaymentGateway>,
}

impl PaymentService {
    pub fn new(
        repository: Arc<PostgresStripeRepository>,
        verifier: Arc<dyn StripeWebhookSignatureVerifier>,
        stripe_gateway: Arc<dyn StripePaymentGateway>,
    ) -> Self {
        Self {
            repository,
            verifier,
            stripe_gateway,
        }
    }

    pub async fn create_embedded_checkout(
        &self,
        input: CreatePaymentAttemptInput,
    ) -> Result<EmbeddedCheckoutResult, ApplicationError> {
        require_checkout_key(&input.actor.machine)?;
        let return_url =
            input
                .return_url
                .as_deref()
                .ok_or_else(|| ApplicationError::Validation {
                    violations: vec![chaos_domain::FieldViolation {
                        field: "return_url",
                        reason: "return_url is required for Stripe Embedded Checkout".into(),
                    }],
                })?;
        self.repository
            .create_attempt(&input.actor, input.order_id)
            .await?;
        let command = self
            .repository
            .prepare_checkout_command(
                &input.actor,
                input.order_id,
                return_url,
                &input.request_id.to_string(),
            )
            .await?;
        let result = self.stripe_gateway.execute(command).await?;
        self.repository
            .record_checkout_result(&input.actor, input.order_id, &result, input.now)
            .await?;
        let client_action = result
            .client_action
            .ok_or_else(|| ApplicationError::Unavailable {
                service: "stripe_checkout_client_secret",
                source: anyhow::anyhow!("Stripe Checkout Session client secret is missing"),
            })?;
        let attempt = self
            .repository
            .get_attempt(&input.actor, input.order_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "order",
                id: input.order_id.as_uuid().to_string(),
            })?;
        Ok(EmbeddedCheckoutResult {
            attempt,
            client_action,
        })
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
                input.order_id,
                input.amount_minor,
            )
            .await
    }

    pub async fn receive_webhook(
        &self,
        provider_account_id: StripeAccountId,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<bool, ApplicationError> {
        let event = self
            .verifier
            .verify(provider_account_id, signature, payload, received_at)
            .await?;
        self.repository.ingest_webhook(&event).await
    }
}

pub struct PaymentWorkers {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<PostgresStripeRepository>,
    stripe_gateway: Arc<dyn StripePaymentGateway>,
}

impl PaymentWorkers {
    pub fn new(
        queue: Arc<dyn IntegrationQueue>,
        repository: Arc<PostgresStripeRepository>,
        stripe_gateway: Arc<dyn StripePaymentGateway>,
    ) -> Self {
        Self {
            queue,
            repository,
            stripe_gateway,
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
                .execute_stripe_job(job, now)
                .await
                .map_err(|error| error.to_string());
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

    async fn execute_stripe_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let command = self.repository.prepare_stripe_command(job).await?;
        let result = self.stripe_gateway.execute(command).await?;
        self.repository
            .record_stripe_result(job, &result, now)
            .await?;
        Ok(())
    }
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

fn require_stripe_account_administrator(actor: StoreActor) -> Result<(), ApplicationError> {
    if actor.role() == StoreRole::Owner {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn stripe_account_not_found(id: StripeAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "stripe_account",
        id: id.as_uuid().to_string(),
    }
}
