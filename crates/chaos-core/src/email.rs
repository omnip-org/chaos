use std::{collections::HashMap, sync::Arc};

use crate::{
    ApplicationError,
    adapters::postgres::PostgresEmailRepository,
    contracts::{
        EmailProvider, EmailWebhookVerifier, IntegrationQueue, ProviderAccountReader,
        VerifiedWebhookEvent, WebhookProcessingResult,
    },
};
use time::OffsetDateTime;
use uuid::Uuid;

pub struct ReceiveEmailWebhook<'a> {
    pub provider: &'a str,
    pub provider_account_id: Uuid,
    pub message_id: &'a str,
    pub timestamp: &'a str,
    pub signature: &'a str,
    pub payload: &'a [u8],
    pub received_at: OffsetDateTime,
}

/// Verifies provider-specific email webhooks, then hands a canonical event to
/// the shared Integration inbox. It never writes provider-specific webhook
/// rows itself.
pub struct EmailWebhooks {
    accounts: Arc<dyn ProviderAccountReader>,
    inbox: Arc<dyn crate::contracts::WebhookInbox>,
    verifiers: HashMap<String, Arc<dyn EmailWebhookVerifier>>,
}

impl EmailWebhooks {
    pub fn new(
        accounts: Arc<dyn ProviderAccountReader>,
        inbox: Arc<dyn crate::contracts::WebhookInbox>,
        verifiers: impl IntoIterator<Item = Arc<dyn EmailWebhookVerifier>>,
    ) -> Self {
        Self {
            accounts,
            inbox,
            verifiers: verifiers
                .into_iter()
                .map(|verifier| (verifier.name().to_owned(), verifier))
                .collect(),
        }
    }

    pub async fn receive(
        &self,
        request: ReceiveEmailWebhook<'_>,
    ) -> Result<bool, ApplicationError> {
        let (_, secret) = self
            .accounts
            .resolve_webhook_secret("email", request.provider, request.provider_account_id)
            .await?
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "email_provider_account",
                id: request.provider_account_id.to_string(),
            })?;
        let verifier =
            self.verifiers
                .get(request.provider)
                .ok_or_else(|| ApplicationError::NotFound {
                    resource: "email_provider",
                    id: request.provider.to_owned(),
                })?;
        let event = verifier
            .verify(
                &secret,
                request.message_id,
                request.timestamp,
                request.signature,
                request.payload,
                request.received_at,
            )
            .await?;
        self.inbox
            .record(VerifiedWebhookEvent {
                provider_account_id: request.provider_account_id,
                capability: "email".into(),
                provider: request.provider.into(),
                provider_event_id: event.provider_event_id,
                provider_event_type: event.provider_event_type,
                normalized_event_type: event.normalized_event_type,
                payload: event.payload,
                aggregate_type: Some("email".into()),
                aggregate_id: None,
                verified_at: event.received_at,
            })
            .await
    }
}

/// Email is an integration consumer, not an Order state machine. It consumes
/// `order.confirmed` events and owns provider retries through the shared
/// integration outbox lease.
pub struct EmailWorkers {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<PostgresEmailRepository>,
    providers: HashMap<String, Arc<dyn EmailProvider>>,
}

impl EmailWorkers {
    pub fn new(
        queue: Arc<dyn IntegrationQueue>,
        repository: Arc<PostgresEmailRepository>,
        providers: impl IntoIterator<Item = Arc<dyn EmailProvider>>,
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
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_outbox("chaos_email_commands", limit)
            .await?;
        for job in &jobs {
            let result = self.execute(job).await.map_err(|error| error.to_string());
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
        let jobs = self.queue.claim_webhooks("email", limit).await?;
        for job in &jobs {
            // Delivery provider events are durably recorded in Integration.
            // A later notification projection can attach them to a delivery
            // row without changing the inbox or retry protocol.
            let result = if job.normalized_event_type.is_some() {
                WebhookProcessingResult::Processed
            } else {
                WebhookProcessingResult::Unsupported {
                    reason: format!(
                        "unsupported {} webhook {}",
                        job.provider.as_deref().unwrap_or("email provider"),
                        job.provider_event_type.as_deref().unwrap_or("unknown")
                    ),
                }
            };
            self.queue
                .finish_webhook(job.id, job.attempts, result, now)
                .await?;
        }
        Ok(jobs.len())
    }

    async fn execute(&self, job: &crate::contracts::QueueJob) -> Result<(), ApplicationError> {
        if job.internal_event_type.as_deref() != Some("order.confirmed") {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "unsupported email event {}",
                job.internal_event_type.as_deref().unwrap_or("unknown")
            )));
        }
        let (provider, reference, message) =
            self.repository.prepare_order_confirmation(job).await?;
        let provider = self
            .providers
            .get(&provider)
            .ok_or_else(|| ApplicationError::Conflict {
                code: "email_provider_not_supported",
                message: "the configured Email provider has no adapter",
            })?;
        provider.send(&reference, message).await.map(|_| ())
    }
}
