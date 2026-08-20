use std::{collections::HashMap, sync::Arc};

use chaos_domain::{
    notifications::{
        NotificationProviderAccount, NotificationProviderAccountId, NotificationSecretReference,
    },
    store::{StoreId, StoreRole},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    ports::{
        AdminActor, EmailDeliveryFailure, EmailDeliveryRepository, EmailMessage, EmailProvider,
        EmailWebhookVerifier, IdempotencyRequest, NotificationProviderAccountConfiguration,
        NotificationProviderAccountDetail, NotificationProviderAccountRepository,
    },
};

pub struct NotificationWorkers {
    repository: Arc<dyn EmailDeliveryRepository>,
    providers: HashMap<String, Arc<dyn EmailProvider>>,
    storefront_public_base_url: String,
}

impl NotificationWorkers {
    pub fn new(
        repository: Arc<dyn EmailDeliveryRepository>,
        providers: impl IntoIterator<Item = Arc<dyn EmailProvider>>,
        storefront_public_base_url: String,
    ) -> Self {
        Self {
            repository,
            providers: providers
                .into_iter()
                .map(|provider| (provider.name().to_owned(), provider))
                .collect(),
            storefront_public_base_url,
        }
    }

    pub async fn run_batch(
        &self,
        now: OffsetDateTime,
        limit: u16,
    ) -> Result<usize, ApplicationError> {
        if self.providers.is_empty() {
            return Ok(0);
        }
        let jobs = self.repository.claim(limit).await?;
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
                        .send(
                            &job.credential_secret_reference,
                            EmailMessage {
                                from: job.sender,
                                to: job.recipient_email,
                                subject,
                                text,
                                html: None,
                                idempotency_key: format!("notification-{}", job.id),
                            },
                        )
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
                .finish(job.id, job.attempts, result, now)
                .await?;
        }
        Ok(count)
    }
}

pub struct NotificationWebhooks {
    repository: Arc<dyn EmailDeliveryRepository>,
    accounts: Arc<dyn NotificationProviderAccountRepository>,
    verifiers: HashMap<String, Arc<dyn EmailWebhookVerifier>>,
}

pub struct ReceiveNotificationWebhook<'a> {
    pub provider: &'a str,
    pub provider_account_id: NotificationProviderAccountId,
    pub message_id: &'a str,
    pub timestamp: &'a str,
    pub signature: &'a str,
    pub payload: &'a [u8],
    pub received_at: OffsetDateTime,
}

impl NotificationWebhooks {
    pub fn new(
        repository: Arc<dyn EmailDeliveryRepository>,
        accounts: Arc<dyn NotificationProviderAccountRepository>,
        verifiers: impl IntoIterator<Item = Arc<dyn EmailWebhookVerifier>>,
    ) -> Self {
        Self {
            repository,
            accounts,
            verifiers: verifiers
                .into_iter()
                .map(|verifier| (verifier.name().to_owned(), verifier))
                .collect(),
        }
    }

    pub async fn receive(
        &self,
        request: ReceiveNotificationWebhook<'_>,
    ) -> Result<bool, ApplicationError> {
        let configuration = self
            .accounts
            .resolve_webhook(request.provider_account_id)
            .await?
            .filter(|value| value.provider == request.provider)
            .ok_or_else(|| ApplicationError::NotFound {
                resource: "notification_provider_account",
                id: request.provider_account_id.as_uuid().to_string(),
            })?;
        let verifier = self
            .verifiers
            .get(request.provider)
            .ok_or(ApplicationError::NotFound {
                resource: "notification_provider",
                id: request.provider.to_owned(),
            })?;
        let event = verifier
            .verify(
                &configuration.webhook_secret_reference,
                request.message_id,
                request.timestamp,
                request.signature,
                request.payload,
                request.received_at,
            )
            .await?;
        self.repository
            .record_webhook(request.provider_account_id, &event)
            .await
    }
}

pub struct ConfigureNotificationProviderInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub provider: String,
    pub display_name: String,
    pub sender: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: String,
    pub enabled: bool,
    pub idempotency: IdempotencyRequest,
}

pub struct NotificationProviderAdministration {
    repository: Arc<dyn NotificationProviderAccountRepository>,
}

impl NotificationProviderAdministration {
    pub fn new(repository: Arc<dyn NotificationProviderAccountRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<NotificationProviderAccountDetail>, ApplicationError> {
        require_owner(&actor)?;
        self.repository.list(actor, store_id).await
    }

    pub async fn configure(
        &self,
        input: ConfigureNotificationProviderInput,
    ) -> Result<NotificationProviderAccountDetail, ApplicationError> {
        require_owner(&input.actor)?;
        let account = NotificationProviderAccount::create(
            input.provider,
            input.display_name,
            input.sender,
            input.enabled,
        )?;
        let configuration = NotificationProviderAccountConfiguration {
            credential_secret_reference: NotificationSecretReference::new(
                input.credential_secret_reference,
            )?,
            webhook_secret_reference: NotificationSecretReference::new(
                input.webhook_secret_reference,
            )?,
        };
        self.repository
            .configure(
                input.actor,
                input.store_id,
                &account,
                &configuration,
                &input.idempotency,
            )
            .await
    }
}

fn require_owner(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(actor) if actor.role() == StoreRole::Owner => Ok(()),
        _ => Err(ApplicationError::Forbidden),
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
