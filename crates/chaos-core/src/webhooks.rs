use std::sync::Arc;

use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::{
        PostgresProviderWebhookAudit, PostgresStripeRepository, ProviderWebhookAuditRow,
    },
    contracts::{IntegrationQueue, PROVIDER_WEBHOOKS_QUEUE, PaymentProviderRegistry},
};

/// Drains `provider_webhooks_queue`. Every job is a pointer
/// (`{webhook_id, store_id}`) into `integration.provider_webhook_audit`;
/// the worker loads that row and applies the event through the capability that
/// owns it. Failures ride PGMQ's own retry/backoff/archive, and
/// `process_webhook_job` is written to be idempotent so a redelivery is safe.
pub struct ProviderWebhookWorker {
    queue: Arc<dyn IntegrationQueue>,
    audit: Arc<PostgresProviderWebhookAudit>,
    payments: Arc<PostgresStripeRepository>,
    payment_providers: Arc<PaymentProviderRegistry>,
}

impl ProviderWebhookWorker {
    pub fn new(
        queue: Arc<dyn IntegrationQueue>,
        audit: Arc<PostgresProviderWebhookAudit>,
        payments: Arc<PostgresStripeRepository>,
        payment_providers: Arc<PaymentProviderRegistry>,
    ) -> Self {
        Self {
            queue,
            audit,
            payments,
            payment_providers,
        }
    }

    pub async fn run_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_topic(PROVIDER_WEBHOOKS_QUEUE, limit)
            .await?;
        for job in &jobs {
            let result = self.process(&job.payload, now).await;
            if let Err(error) = &result {
                tracing::warn!(error = %error, "provider webhook processing failed");
            }
            self.queue
                .finish_topic(
                    PROVIDER_WEBHOOKS_QUEUE,
                    job.msg_id,
                    job.attempts,
                    result.map_err(|error| error.to_string()),
                )
                .await?;
        }
        Ok(jobs.len())
    }

    async fn process(&self, message: &Value, now: OffsetDateTime) -> Result<(), ApplicationError> {
        let webhook_id = message_uuid(message, "webhook_id")?;
        let store_id = message_uuid(message, "store_id")?;
        let Some(row) = self.audit.load(store_id, webhook_id).await? else {
            // The audit row is gone (store deleted); nothing to apply.
            return Ok(());
        };
        if row.processed_at.is_some() {
            // A redelivery after a crash between commit and finish_topic.
            return Ok(());
        }
        match chaos_domain::integration::IntegrationCapability::parse(&row.capability) {
            Some(chaos_domain::integration::IntegrationCapability::Payment) => {
                self.apply_payment(store_id, &row, now).await?
            }
            Some(chaos_domain::integration::IntegrationCapability::Email) => {
                // Verified email provider webhooks have no side effect to apply
                // today; the audit row is the whole record.
            }
            Some(capability) => {
                tracing::warn!(
                    capability = capability.as_str(),
                    "provider webhook for an unhandled capability"
                );
            }
            None => {
                tracing::warn!(
                    capability = %row.capability,
                    "provider webhook contains an unknown integration capability"
                );
            }
        }
        self.audit.mark_processed(store_id, webhook_id, now).await
    }

    async fn apply_payment(
        &self,
        store_id: Uuid,
        row: &ProviderWebhookAuditRow,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let Some(normalized_event_type) = row.normalized_event_type.as_deref() else {
            // Verified but not a shape this version understands. The audit row
            // stands; there is nothing to act on.
            tracing::info!(
                provider = %row.provider,
                provider_event_type = %row.provider_event_type,
                "unsupported payment webhook"
            );
            return Ok(());
        };
        let reconciliation = self
            .payments
            .process_webhook_job(
                store_id,
                normalized_event_type,
                row.provider_account_id,
                &row.payload,
                now,
            )
            .await?;
        if let Some(context) = reconciliation {
            let gateway =
                self.payment_providers
                    .get(&row.provider)
                    .ok_or(ApplicationError::Conflict {
                        code: "payment_provider_not_supported",
                        message: "the configured Payment provider has no adapter",
                    })?;
            let observations = gateway
                .list_refunds(
                    &context.credential_secret_reference,
                    &context.payment_provider_reference,
                )
                .await?;
            self.payments
                .apply_refund_reconciliation(&context, &observations, now)
                .await?;
        }
        Ok(())
    }
}

fn message_uuid(message: &Value, field: &'static str) -> Result<Uuid, ApplicationError> {
    message
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "provider webhook job missing or invalid field {field}"
            ))
        })
}
