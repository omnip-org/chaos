use std::sync::Arc;

use chaos_domain::{
    payments::{CheckoutAttemptId, CheckoutAttemptStatus},
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
        AdminActor, CheckoutAttemptDetail, IntegrationQueue, MachineActor, PaymentClientAction,
        PaymentProviderRegistry, PaymentWebhookVerifierRegistry, QueueJob, RefundDetail,
        ShopperActor, StripeAccountConfiguration, StripeAccountDetail, StripeAccountPage,
        VerifiedWebhookEvent, WebhookInbox, WebhookProcessingResult,
    },
    store::StoreActor,
};

pub struct CreatePaymentAttemptInput {
    pub actor: ShopperActor,
    pub checkout_attempt_id: CheckoutAttemptId,
    pub now: OffsetDateTime,
}

pub struct ResumePaymentAttemptInput {
    pub actor: ShopperActor,
    pub checkout_attempt_id: CheckoutAttemptId,
    pub now: OffsetDateTime,
}

pub struct EmbeddedCheckoutResult {
    pub attempt: CheckoutAttemptDetail,
    pub client_action: PaymentClientAction,
}

pub struct CreateRefundInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub amount_minor: i64,
}

pub struct ReconcileRefundsInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub now: OffsetDateTime,
}

pub struct RefundReconciliationResult {
    pub order_id: OrderId,
    pub refunded_amount_minor: i64,
    pub refunds: Vec<RefundDetail>,
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
    webhook_inbox: Arc<dyn WebhookInbox>,
    webhook_verifiers: Arc<PaymentWebhookVerifierRegistry>,
    payment_providers: Arc<PaymentProviderRegistry>,
}

impl PaymentService {
    pub fn new(
        repository: Arc<PostgresStripeRepository>,
        webhook_inbox: Arc<dyn WebhookInbox>,
        webhook_verifiers: Arc<PaymentWebhookVerifierRegistry>,
        payment_providers: Arc<PaymentProviderRegistry>,
    ) -> Self {
        Self {
            repository,
            webhook_inbox,
            webhook_verifiers,
            payment_providers,
        }
    }

    pub async fn create_embedded_checkout(
        &self,
        input: CreatePaymentAttemptInput,
    ) -> Result<EmbeddedCheckoutResult, ApplicationError> {
        require_checkout_key(&input.actor.machine)?;
        self.open_embedded_checkout(input.actor, input.checkout_attempt_id, input.now)
            .await
    }

    pub async fn resume_embedded_checkout(
        &self,
        input: ResumePaymentAttemptInput,
    ) -> Result<EmbeddedCheckoutResult, ApplicationError> {
        require_checkout_key(&input.actor.machine)?;
        self.open_embedded_checkout(input.actor, input.checkout_attempt_id, input.now)
            .await
    }

    pub async fn get_checkout_attempt(
        &self,
        actor: &ShopperActor,
        attempt_id: CheckoutAttemptId,
    ) -> Result<CheckoutAttemptDetail, ApplicationError> {
        require_checkout_key(&actor.machine)?;
        self.repository
            .get_checkout_attempt(actor, attempt_id)
            .await?
            .ok_or_else(|| checkout_attempt_not_found(attempt_id))
    }

    pub async fn list_checkout_attempts(
        &self,
        actor: &ShopperActor,
    ) -> Result<Vec<CheckoutAttemptDetail>, ApplicationError> {
        require_checkout_key(&actor.machine)?;
        self.repository.list_checkout_attempts(actor).await
    }

    async fn open_embedded_checkout(
        &self,
        actor: ShopperActor,
        checkout_attempt_id: CheckoutAttemptId,
        now: OffsetDateTime,
    ) -> Result<EmbeddedCheckoutResult, ApplicationError> {
        let attempt = self
            .repository
            .get_checkout_attempt_payment(&actor, checkout_attempt_id)
            .await?
            .ok_or_else(|| checkout_attempt_not_found(checkout_attempt_id))?;
        ensure_checkout_attempt_open(&attempt.detail, now)?;
        if let Some(client_action) = attempt.client_action()? {
            return Ok(EmbeddedCheckoutResult {
                attempt: attempt.detail,
                client_action,
            });
        }

        let provider = self
            .payment_providers
            .get(&attempt.provider)
            .ok_or_else(payment_provider_not_supported)?;
        let command = self
            .repository
            .prepare_checkout_command(&actor, &attempt)
            .await?;
        let result = provider.execute(command).await?;
        if result.client_action.is_none() {
            self.repository
                .fail_checkout_attempt(
                    &actor,
                    checkout_attempt_id,
                    "checkout_client_action_missing",
                    now,
                )
                .await?;
            return Err(checkout_client_action_missing());
        }
        self.repository
            .record_checkout_result(&actor, checkout_attempt_id, &result, now)
            .await?;
        let attempt = self
            .repository
            .get_checkout_attempt_payment(&actor, checkout_attempt_id)
            .await?
            .ok_or_else(|| checkout_attempt_not_found(checkout_attempt_id))?;
        let client_action = attempt
            .client_action()?
            .ok_or_else(checkout_client_action_missing)?;
        Ok(EmbeddedCheckoutResult {
            attempt: attempt.detail,
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

    pub async fn reconcile_refunds(
        &self,
        input: ReconcileRefundsInput,
    ) -> Result<RefundReconciliationResult, ApplicationError> {
        require_payment_operator(&input.actor)?;
        let context = self
            .repository
            .prepare_refund_reconciliation(&input.actor, input.store_id, input.order_id)
            .await?;
        let provider = self
            .payment_providers
            .get("stripe")
            .ok_or_else(payment_provider_not_supported)?;
        let observations = provider
            .list_refunds(
                &context.credential_secret_reference,
                &context.payment_provider_reference,
            )
            .await?;
        let (refunded_amount_minor, refunds) = self
            .repository
            .apply_refund_reconciliation(&context, &observations, input.now)
            .await?;
        Ok(RefundReconciliationResult {
            order_id: input.order_id,
            refunded_amount_minor,
            refunds,
        })
    }

    pub async fn receive_webhook(
        &self,
        provider_account_id: StripeAccountId,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<bool, ApplicationError> {
        self.receive_provider_webhook(
            "stripe",
            provider_account_id.as_uuid(),
            signature,
            payload,
            received_at,
        )
        .await
    }

    pub async fn receive_provider_webhook(
        &self,
        provider: &str,
        provider_account_id: Uuid,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<bool, ApplicationError> {
        let verifier =
            self.webhook_verifiers
                .get(provider)
                .ok_or_else(|| ApplicationError::NotFound {
                    resource: "payment_provider",
                    id: provider.to_owned(),
                })?;
        let event = verifier
            .verify(provider_account_id, signature, payload, received_at)
            .await?;
        self.webhook_inbox
            .record(VerifiedWebhookEvent {
                provider_account_id: event.provider_account_id,
                capability: "payment".into(),
                provider: provider.to_owned(),
                provider_event_id: event.provider_event_id,
                provider_event_type: event.provider_event_type,
                normalized_event_type: event.normalized_event_type,
                payload: event.payload,
                aggregate_type: event.order_id.map(|_| "order".into()),
                aggregate_id: event.order_id,
                verified_at: event.verified_at,
            })
            .await
    }
}

pub struct PaymentWorkers {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<PostgresStripeRepository>,
    payment_providers: Arc<PaymentProviderRegistry>,
}

impl PaymentWorkers {
    pub fn new(
        queue: Arc<dyn IntegrationQueue>,
        repository: Arc<PostgresStripeRepository>,
        payment_providers: Arc<PaymentProviderRegistry>,
    ) -> Self {
        Self {
            queue,
            repository,
            payment_providers,
        }
    }

    pub async fn run_outbox_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_outbox("chaos_payment_commands", limit)
            .await?;
        for job in &jobs {
            let result = self
                .execute_payment_job(job, now)
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
        let jobs = self.queue.claim_webhooks("payment", limit).await?;
        for job in &jobs {
            let result = if job.normalized_event_type.is_none() {
                WebhookProcessingResult::Unsupported {
                    reason: format!(
                        "unsupported {} webhook {}",
                        job.provider.as_deref().unwrap_or("payment provider"),
                        job.provider_event_type.as_deref().unwrap_or("unknown")
                    ),
                }
            } else {
                match self.repository.process_webhook_job(job, now).await {
                    Ok(Some(context)) => {
                        let provider_name = job.provider.as_deref().unwrap_or("stripe");
                        let result = async {
                            let provider = self
                                .payment_providers
                                .get(provider_name)
                                .ok_or_else(payment_provider_not_supported)?;
                            let observations = provider
                                .list_refunds(
                                    &context.credential_secret_reference,
                                    &context.payment_provider_reference,
                                )
                                .await?;
                            self.repository
                                .apply_refund_reconciliation(&context, &observations, now)
                                .await
                        }
                        .await;
                        match result {
                            Ok(_) => WebhookProcessingResult::Processed,
                            Err(error) => WebhookProcessingResult::Failed {
                                reason: error.to_string(),
                            },
                        }
                    }
                    Ok(None) => WebhookProcessingResult::Processed,
                    Err(error) => WebhookProcessingResult::Failed {
                        reason: error.to_string(),
                    },
                }
            };
            self.queue
                .finish_webhook(job.id, job.attempts, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    async fn execute_payment_job(
        &self,
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let command = self.repository.prepare_payment_command(job).await?;
        let provider_name = job.provider.as_deref().unwrap_or("stripe");
        let provider = self
            .payment_providers
            .get(provider_name)
            .ok_or_else(payment_provider_not_supported)?;
        let result = provider.execute(command).await?;
        self.repository
            .record_payment_result(job, &result, now)
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

fn payment_provider_not_supported() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_not_supported",
        message: "the configured Payment provider has no adapter",
    }
}

fn ensure_checkout_attempt_open(
    attempt: &CheckoutAttemptDetail,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    if attempt.expires_at <= now {
        return Err(ApplicationError::Conflict {
            code: "checkout_attempt_expired",
            message: "the Checkout Attempt has expired; start a new checkout from the active Cart",
        });
    }
    if matches!(
        attempt.status,
        CheckoutAttemptStatus::Creating | CheckoutAttemptStatus::Open
    ) {
        Ok(())
    } else {
        Err(ApplicationError::Conflict {
            code: "checkout_attempt_not_open",
            message: "the Checkout Attempt is no longer available for payment",
        })
    }
}

fn checkout_attempt_not_found(attempt_id: CheckoutAttemptId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "checkout_attempt",
        id: attempt_id.as_uuid().to_string(),
    }
}

fn checkout_client_action_missing() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "stripe_checkout_client_secret",
        source: anyhow::anyhow!("Stripe Checkout Session client secret is missing"),
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
