use std::{collections::HashMap, sync::Arc};

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    ApplicationError,
    ports::{
        EmailDeliveryFailure, EmailDeliveryRepository, EmailMessage, EmailProvider,
        EmailWebhookVerifier,
    },
};

const WORKER_LEASE_TIMEOUT: Duration = Duration::minutes(1);

pub struct NotificationWorkers {
    repository: Arc<dyn EmailDeliveryRepository>,
    providers: HashMap<String, Arc<dyn EmailProvider>>,
    email_from: String,
    storefront_public_base_url: String,
}

impl NotificationWorkers {
    pub fn new(
        repository: Arc<dyn EmailDeliveryRepository>,
        providers: impl IntoIterator<Item = Arc<dyn EmailProvider>>,
        email_from: String,
        storefront_public_base_url: String,
    ) -> Self {
        Self {
            repository,
            providers: providers
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
            email_from,
            storefront_public_base_url,
        }
    }

    pub async fn run_batch(
        &self,
        worker_id: Uuid,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        if self.providers.is_empty() {
            return Ok(0);
        }
        let jobs = self
            .repository
            .claim(worker_id, limit, now, now - WORKER_LEASE_TIMEOUT)
            .await?;
        let count = jobs.len();
        for job in jobs {
            let result = match self.providers.get(&job.provider) {
                Some(provider) => match render(
                    &job.template_key,
                    job.template_version,
                    &job.template_payload,
                    &self.storefront_public_base_url,
                ) {
                    Ok((subject, text)) => provider
                        .send(EmailMessage {
                            from: self.email_from.clone(),
                            to: job.recipient_email,
                            subject,
                            text,
                            html: None,
                            idempotency_key: format!("notification-{}", job.id),
                        })
                        .await
                        .map_err(classify_failure),
                    Err(error) => Err(EmailDeliveryFailure {
                        retryable: false,
                        message: error.to_string(),
                    }),
                },
                None => Err(EmailDeliveryFailure {
                    retryable: false,
                    message: "email provider is not configured".into(),
                }),
            };
            self.repository
                .finish(worker_id, job.id, result, now)
                .await?;
        }
        Ok(count)
    }
}

pub struct NotificationWebhooks {
    repository: Arc<dyn EmailDeliveryRepository>,
    verifiers: HashMap<String, Arc<dyn EmailWebhookVerifier>>,
}

impl NotificationWebhooks {
    pub fn new(
        repository: Arc<dyn EmailDeliveryRepository>,
        verifiers: impl IntoIterator<Item = Arc<dyn EmailWebhookVerifier>>,
    ) -> Self {
        Self {
            repository,
            verifiers: verifiers
                .into_iter()
                .map(|verifier| (verifier.name().to_owned(), verifier))
                .collect(),
        }
    }

    pub async fn receive(
        &self,
        provider: &str,
        message_id: &str,
        timestamp: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<bool, ApplicationError> {
        let verifier = self
            .verifiers
            .get(provider)
            .ok_or(ApplicationError::NotFound {
                resource: "notification_provider",
                id: provider.to_owned(),
            })?;
        let event = verifier
            .verify(message_id, timestamp, signature, payload, received_at)
            .await?;
        self.repository.record_webhook(&event).await
    }
}

fn render(
    template_key: &str,
    template_version: u32,
    payload: &serde_json::Value,
    storefront_public_base_url: &str,
) -> Result<(String, String), ApplicationError> {
    match (template_key, template_version) {
        ("order_confirmation", 1) => {
            let order_number = payload
                .get("order_number")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(invalid_template_payload)?;
            let amount = payload
                .get("total_amount_minor")
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(invalid_template_payload)?;
            let currency = payload
                .get("currency")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(invalid_template_payload)?;
            let tracking_key = payload
                .get("tracking_key")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(invalid_template_payload)?;
            let tracking_url = format!(
                "{}/orders/track#{tracking_key}",
                storefront_public_base_url.trim_end_matches('/')
            );
            Ok((
                "Your order is confirmed".into(),
                format!(
                    "Your order {order_number} is confirmed. Total: {amount} minor units {currency}. Track your order: {tracking_url}"
                ),
            ))
        }
        _ => Err(ApplicationError::Conflict {
            code: "notification_template_not_supported",
            message: "The notification template version is not supported",
        }),
    }
}

fn classify_failure(error: ApplicationError) -> EmailDeliveryFailure {
    EmailDeliveryFailure {
        retryable: matches!(
            error,
            ApplicationError::Unavailable { .. } | ApplicationError::Unexpected(_)
        ),
        message: error.to_string(),
    }
}

fn invalid_template_payload() -> ApplicationError {
    ApplicationError::Conflict {
        code: "invalid_notification_template_payload",
        message: "The notification template payload is invalid",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::render;

    #[test]
    fn order_confirmation_uses_display_number_and_chaos_tracking_fragment() {
        let (_, text) = render(
            "order_confirmation",
            1,
            &json!({
                "order_number": "W-20260820-7K4M9Q2D",
                "tracking_key": "otk_secret",
                "total_amount_minor": 1250,
                "currency": "USD"
            }),
            "https://shop.example.com/",
        )
        .unwrap();
        assert!(text.contains("W-20260820-7K4M9Q2D"));
        assert!(text.contains("https://shop.example.com/orders/track#otk_secret"));
    }
}
