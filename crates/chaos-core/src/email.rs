use std::{collections::HashMap, sync::Arc};

use crate::{
    ApplicationError,
    adapters::postgres::{EmailBrandWrite, EmailProviderAccountWrite, PostgresEmailRepository},
    contracts::{
        EmailAccountConfiguration, EmailBrandDetail, EmailProvider, EmailProviderAccountDetail,
        EmailProviderAccountPage, EmailWebhookVerifier, IntegrationQueue, ProviderAccountReader,
        VerifiedWebhookEvent, WebhookProcessingResult,
    },
    store::StoreActor,
};
use chaos_domain::{
    identity::Email,
    store::{StoreId, StoreRole},
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

pub struct CreateEmailProviderAccountInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: Option<String>,
    pub configuration: EmailAccountConfiguration,
    pub enabled: bool,
}

pub struct UpdateEmailProviderAccountInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub id: Uuid,
    pub display_name: String,
    pub credential_secret_reference: String,
    pub webhook_secret_reference: Option<String>,
    pub configuration: EmailAccountConfiguration,
    pub enabled: bool,
}

pub struct ConfigureEmailBrandInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
    pub brand_name: Option<String>,
    pub logo_url: Option<String>,
    pub primary_color: String,
    pub accent_color: String,
    pub background_color: String,
    pub surface_color: String,
    pub text_color: String,
    pub muted_text_color: String,
    pub support_email: Option<String>,
    pub support_url: Option<String>,
}

pub struct ResetEmailBrandInput {
    pub actor: StoreActor,
    pub store_id: StoreId,
}

struct EmailBrandFields {
    brand_name: Option<String>,
    logo_url: Option<String>,
    primary_color: String,
    accent_color: String,
    background_color: String,
    surface_color: String,
    text_color: String,
    muted_text_color: String,
    support_email: Option<String>,
    support_url: Option<String>,
}

/// Owner-facing administration for Store-owned email provider accounts.
/// Provider credentials are supplied as opaque secret references; the raw
/// values are never accepted by this service or returned from read methods.
pub struct EmailProviderAccountAdministration {
    repository: Arc<PostgresEmailRepository>,
}

impl EmailProviderAccountAdministration {
    pub fn new(repository: Arc<PostgresEmailRepository>) -> Self {
        Self { repository }
    }

    pub async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
    ) -> Result<EmailProviderAccountPage, ApplicationError> {
        self.repository
            .list_provider_accounts(actor, store_id, after, limit)
            .await
    }

    pub async fn get(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        id: Uuid,
    ) -> Result<EmailProviderAccountDetail, ApplicationError> {
        self.repository
            .get_provider_account(actor, store_id, id)
            .await?
            .ok_or_else(|| email_provider_account_not_found(id))
    }

    pub async fn create(
        &self,
        input: CreateEmailProviderAccountInput,
    ) -> Result<EmailProviderAccountDetail, ApplicationError> {
        require_email_provider_account_administrator(&input.actor)?;
        let display_name = validate_display_name(input.display_name)?;
        let credential_secret_reference = validate_secret_reference(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook_secret_reference = input
            .webhook_secret_reference
            .map(|value| validate_secret_reference("webhook_secret_reference", value))
            .transpose()?;
        let configuration = validate_configuration(input.configuration)?;
        self.repository
            .create_provider_account(
                input.actor,
                input.store_id,
                EmailProviderAccountWrite {
                    display_name: &display_name,
                    credential_secret_reference: &credential_secret_reference,
                    webhook_secret_reference: webhook_secret_reference.as_deref(),
                    configuration: &configuration,
                    enabled: input.enabled,
                },
            )
            .await
    }

    pub async fn update(
        &self,
        input: UpdateEmailProviderAccountInput,
    ) -> Result<EmailProviderAccountDetail, ApplicationError> {
        require_email_provider_account_administrator(&input.actor)?;
        let display_name = validate_display_name(input.display_name)?;
        let credential_secret_reference = validate_secret_reference(
            "credential_secret_reference",
            input.credential_secret_reference,
        )?;
        let webhook_secret_reference = input
            .webhook_secret_reference
            .map(|value| validate_secret_reference("webhook_secret_reference", value))
            .transpose()?;
        let configuration = validate_configuration(input.configuration)?;
        self.repository
            .update_provider_account(
                input.actor,
                input.store_id,
                input.id,
                EmailProviderAccountWrite {
                    display_name: &display_name,
                    credential_secret_reference: &credential_secret_reference,
                    webhook_secret_reference: webhook_secret_reference.as_deref(),
                    configuration: &configuration,
                    enabled: input.enabled,
                },
            )
            .await
    }
}

/// Store-scoped brand administration for the platform-owned transactional
/// email templates. An Email Provider Account without a `brand` key uses the
/// built-in brand defaults and the Store name as its brand name.
pub struct EmailBrandAdministration {
    repository: Arc<PostgresEmailRepository>,
}

impl EmailBrandAdministration {
    pub fn new(repository: Arc<PostgresEmailRepository>) -> Self {
        Self { repository }
    }

    pub async fn get(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<EmailBrandDetail, ApplicationError> {
        self.repository.get_email_brand(actor, store_id).await
    }

    pub async fn configure(
        &self,
        input: ConfigureEmailBrandInput,
    ) -> Result<EmailBrandDetail, ApplicationError> {
        let ConfigureEmailBrandInput {
            actor,
            store_id,
            brand_name,
            logo_url,
            primary_color,
            accent_color,
            background_color,
            surface_color,
            text_color,
            muted_text_color,
            support_email,
            support_url,
        } = input;
        require_email_provider_account_administrator(&actor)?;
        let configuration = validate_brand_configuration(EmailBrandFields {
            brand_name,
            logo_url,
            primary_color,
            accent_color,
            background_color,
            surface_color,
            text_color,
            muted_text_color,
            support_email,
            support_url,
        })?;
        self.repository
            .upsert_email_brand(actor, store_id, &configuration)
            .await
    }

    pub async fn reset(
        &self,
        input: ResetEmailBrandInput,
    ) -> Result<EmailBrandDetail, ApplicationError> {
        require_email_provider_account_administrator(&input.actor)?;
        self.repository
            .reset_email_brand(input.actor, input.store_id)
            .await
    }
}

fn require_email_provider_account_administrator(
    actor: &StoreActor,
) -> Result<(), ApplicationError> {
    if actor.role() == StoreRole::Owner {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn email_provider_account_not_found(id: Uuid) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "email_provider_account",
        id: id.to_string(),
    }
}

fn validate_display_name(value: String) -> Result<String, ApplicationError> {
    if value.trim().is_empty() || value.chars().count() > 120 || value.chars().any(char::is_control)
    {
        return Err(validation(
            "display_name",
            "must contain bounded printable text",
        ));
    }
    Ok(value)
}

fn validate_configuration(
    configuration: EmailAccountConfiguration,
) -> Result<EmailAccountConfiguration, ApplicationError> {
    let from_email = Email::parse(configuration.from_email)
        .map_err(|_| validation("from_email", "must be a valid email address"))?;
    let from_name = configuration
        .from_name
        .map(|value| {
            let value = value.trim();
            if value.is_empty()
                || value.chars().count() > 120
                || value.chars().any(char::is_control)
            {
                Err(validation(
                    "from_name",
                    "must contain bounded printable text when provided",
                ))
            } else {
                Ok(value.to_owned())
            }
        })
        .transpose()?;
    Ok(EmailAccountConfiguration {
        from_email: from_email.as_str().to_owned(),
        from_name,
    })
}

fn validate_secret_reference(
    field: &'static str,
    value: String,
) -> Result<String, ApplicationError> {
    let valid_encrypted = value.strip_prefix("enc://").is_some_and(|encoded| {
        !encoded.is_empty()
            && value.len() <= 32_768
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    });
    let valid_environment = value
        .strip_prefix("env://CHAOS_INTEGRATION_SECRET_")
        .is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= 96
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        });
    if valid_encrypted || valid_environment {
        Ok(value)
    } else {
        Err(validation(
            field,
            "must be an enc:// reference or an env://CHAOS_INTEGRATION_SECRET_* reference; do not pass plaintext credentials",
        ))
    }
}

fn validate_brand_configuration(
    fields: EmailBrandFields,
) -> Result<EmailBrandWrite, ApplicationError> {
    let EmailBrandFields {
        brand_name,
        logo_url,
        primary_color,
        accent_color,
        background_color,
        surface_color,
        text_color,
        muted_text_color,
        support_email,
        support_url,
    } = fields;
    let brand_name = brand_name
        .map(|value| validate_optional_text("brand_name", &value, 120))
        .transpose()?;
    let logo_url = logo_url
        .map(|value| validate_https_url("logo_url", &value))
        .transpose()?;
    let support_email = support_email
        .map(|value| {
            Email::parse(value)
                .map(|email| email.as_str().to_owned())
                .map_err(|_| validation("support_email", "must be a valid email address"))
        })
        .transpose()?;
    let support_url = support_url
        .map(|value| validate_https_url("support_url", &value))
        .transpose()?;
    Ok(EmailBrandWrite {
        brand_name,
        logo_url,
        primary_color: validate_color("primary_color", &primary_color)?,
        accent_color: validate_color("accent_color", &accent_color)?,
        background_color: validate_color("background_color", &background_color)?,
        surface_color: validate_color("surface_color", &surface_color)?,
        text_color: validate_color("text_color", &text_color)?,
        muted_text_color: validate_color("muted_text_color", &muted_text_color)?,
        support_email,
        support_url,
    })
}

fn validate_optional_text(
    field: &'static str,
    value: &str,
    max_chars: usize,
) -> Result<String, ApplicationError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max_chars || value.chars().any(char::is_control)
    {
        return Err(validation(
            field,
            "must contain bounded printable text when provided",
        ));
    }
    Ok(value.to_owned())
}

fn validate_https_url(field: &'static str, value: &str) -> Result<String, ApplicationError> {
    let value = value.trim();
    let parsed = url::Url::parse(value)
        .map_err(|_| validation(field, "must be a valid public HTTPS URL when provided"))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || value.len() > 2048
        || value.chars().any(char::is_control)
    {
        return Err(validation(
            field,
            "must be a valid public HTTPS URL when provided",
        ));
    }
    Ok(value.to_owned())
}

fn validate_color(field: &'static str, value: &str) -> Result<String, ApplicationError> {
    let value = value.trim();
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(validation(
            field,
            "must be a six-digit hexadecimal color such as #175CD3",
        ));
    }
    Ok(value.to_ascii_uppercase())
}

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}

/// Email is an integration consumer, not an Order state machine. It consumes
/// `payment.completed` (`notification_email_queue`, see
/// `migrations/0011_topic_routing.sql`) and owns provider retries through
/// PGMQ's own message lifecycle.
pub struct EmailWorkers {
    queue: Arc<dyn IntegrationQueue>,
    repository: Arc<PostgresEmailRepository>,
    providers: HashMap<String, Arc<dyn EmailProvider>>,
}

const NOTIFICATION_EMAIL_QUEUE: &str = "notification_email_queue";

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

    pub async fn run_outbox_batch(&self, limit: u16) -> Result<usize, ApplicationError> {
        let jobs = self
            .queue
            .claim_topic(NOTIFICATION_EMAIL_QUEUE, limit)
            .await?;
        for job in &jobs {
            let result = self
                .execute(&job.payload)
                .await
                .map_err(|error| error.to_string());
            self.queue
                .finish_topic(NOTIFICATION_EMAIL_QUEUE, job.msg_id, job.attempts, result)
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

    async fn execute(&self, payload: &serde_json::Value) -> Result<(), ApplicationError> {
        let store_id = topic_uuid(payload, "store_id")?;
        let order_id = topic_uuid(payload, "order_id")?;
        let Some((provider, reference, message)) = self
            .repository
            .prepare_order_confirmation(store_id, order_id)
            .await?
        else {
            // No contact email to send to (yet). This is a terminal outcome,
            // not a transient failure: nothing will change on retry.
            return Ok(());
        };
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

fn topic_uuid(payload: &serde_json::Value, field: &'static str) -> Result<Uuid, ApplicationError> {
    payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "commerce event message missing or invalid field {field}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        EmailAccountConfiguration, EmailBrandFields, validate_brand_configuration,
        validate_configuration, validate_secret_reference,
    };

    #[test]
    fn normalizes_sender_configuration() {
        let configuration = validate_configuration(EmailAccountConfiguration {
            from_email: " Orders@Example.COM ".into(),
            from_name: Some("  Example Store  ".into()),
        })
        .unwrap();

        assert_eq!(configuration.from_email, "orders@example.com");
        assert_eq!(configuration.from_name.as_deref(), Some("Example Store"));
        assert_eq!(configuration.sender(), "Example Store <orders@example.com>");
    }

    #[test]
    fn rejects_plaintext_email_provider_credentials() {
        assert!(
            validate_secret_reference("credential_secret_reference", "re_live_key".into()).is_err()
        );
        assert!(
            validate_secret_reference(
                "credential_secret_reference",
                "enc://encrypted-reference_1".into()
            )
            .is_ok()
        );
        assert!(
            validate_secret_reference(
                "credential_secret_reference",
                "env://CHAOS_INTEGRATION_SECRET_RESEND".into()
            )
            .is_ok()
        );
        assert!(
            validate_secret_reference(
                "credential_secret_reference",
                "env://CHAOS_PAYMENT_SECRET_RESEND".into()
            )
            .is_err()
        );
    }

    #[test]
    fn validates_store_email_branding() {
        let configuration = validate_brand_configuration(EmailBrandFields {
            brand_name: Some(" Example Store ".into()),
            logo_url: Some("https://cdn.example/logo.png".into()),
            primary_color: "#175cd3".into(),
            accent_color: "#0e7490".into(),
            background_color: "#f4f6f8".into(),
            surface_color: "#ffffff".into(),
            text_color: "#17202a".into(),
            muted_text_color: "#667085".into(),
            support_email: Some("support@example.com".into()),
            support_url: Some("https://example.com/help".into()),
        })
        .unwrap();

        assert_eq!(configuration.brand_name.as_deref(), Some("Example Store"));
        assert_eq!(configuration.primary_color, "#175CD3");
        assert!(
            validate_brand_configuration(EmailBrandFields {
                brand_name: None,
                logo_url: Some("http://cdn.example/logo.png".into()),
                primary_color: "#175CD3".into(),
                accent_color: "#0E7490".into(),
                background_color: "#F4F6F8".into(),
                surface_color: "#FFFFFF".into(),
                text_color: "#17202A".into(),
                muted_text_color: "#667085".into(),
                support_email: None,
                support_url: None,
            })
            .is_err()
        );
    }
}
