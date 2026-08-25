use std::sync::Arc;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::{Client, StatusCode, Url, header};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    contracts::{
        EmailDelivery, EmailMessage, EmailProvider, EmailWebhookVerifier,
        IntegrationSecretResolver, VerifiedEmailWebhook,
    },
};

#[derive(Clone)]
pub struct ResendEmailProvider {
    client: Client,
    api_base_url: Url,
    secrets: Arc<dyn IntegrationSecretResolver>,
}

impl ResendEmailProvider {
    pub fn new(
        api_base_url: Url,
        secrets: Arc<dyn IntegrationSecretResolver>,
        timeout: std::time::Duration,
    ) -> anyhow::Result<Self> {
        let local_host = api_base_url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        anyhow::ensure!(
            api_base_url.scheme() == "https" || local_host,
            "RESEND_API_BASE_URL must use HTTPS except on localhost or loopback"
        );
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            client: Client::builder().timeout(timeout).build()?,
            api_base_url,
            secrets,
        })
    }
}

#[derive(Serialize)]
struct SendRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<&'a str>,
}

#[derive(Deserialize)]
struct SendResponse {
    id: String,
}

#[derive(Deserialize)]
struct WebhookPayload {
    #[serde(rename = "type")]
    provider_event_type: String,
}

#[async_trait]
impl EmailWebhookVerifier for ResendEmailProvider {
    fn name(&self) -> &'static str {
        "resend"
    }

    async fn verify(
        &self,
        webhook_secret_reference: &str,
        message_id: &str,
        timestamp: &str,
        signature: &str,
        payload: &[u8],
        received_at: OffsetDateTime,
    ) -> Result<VerifiedEmailWebhook, ApplicationError> {
        let secret = self.secrets.resolve(webhook_secret_reference).await?;
        let encoded = secret
            .expose_secret()
            .strip_prefix("whsec_")
            .unwrap_or_else(|| secret.expose_secret());
        let key = STANDARD
            .decode(encoded)
            .map_err(|_| ApplicationError::Unauthorized)?;
        if key.len() < 16
            || payload.len() > 64 * 1024
            || message_id.is_empty()
            || message_id.len() > 255
        {
            return Err(ApplicationError::Unauthorized);
        }
        let timestamp_seconds = timestamp
            .parse::<i64>()
            .map_err(|_| ApplicationError::Unauthorized)?;
        let signed_at = OffsetDateTime::from_unix_timestamp(timestamp_seconds)
            .map_err(|_| ApplicationError::Unauthorized)?;
        if (received_at - signed_at).abs() > Duration::minutes(5) {
            return Err(ApplicationError::Unauthorized);
        }
        let signed = [
            message_id.as_bytes(),
            b".",
            timestamp.as_bytes(),
            b".",
            payload,
        ]
        .concat();
        let valid = signature.split_whitespace().any(|candidate| {
            let Some(encoded) = candidate.strip_prefix("v1,") else {
                return false;
            };
            let Ok(expected) = STANDARD.decode(encoded) else {
                return false;
            };
            let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&key) else {
                return false;
            };
            mac.update(&signed);
            mac.verify_slice(&expected).is_ok()
        });
        if !valid {
            return Err(ApplicationError::Unauthorized);
        }
        let parsed: WebhookPayload =
            serde_json::from_slice(payload).map_err(|error| ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "body",
                    reason: format!("must be a valid Resend webhook: {error}"),
                }],
            })?;
        if parsed.provider_event_type.trim().is_empty()
            || parsed.provider_event_type.chars().count() > 255
        {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "body.type",
                    reason: "must contain a non-empty provider event type".into(),
                }],
            });
        }
        let normalized_event_type = matches!(
            parsed.provider_event_type.as_str(),
            "email.sent"
                | "email.delivered"
                | "email.delivery_delayed"
                | "email.bounced"
                | "email.complained"
                | "email.suppressed"
        )
        .then(|| parsed.provider_event_type.clone());
        Ok(VerifiedEmailWebhook {
            provider_event_id: message_id.to_owned(),
            provider_event_type: parsed.provider_event_type,
            normalized_event_type,
            payload: serde_json::from_slice(payload)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
            received_at,
        })
    }
}

#[async_trait]
impl EmailProvider for ResendEmailProvider {
    fn name(&self) -> &'static str {
        "resend"
    }

    async fn send(
        &self,
        credential_secret_reference: &str,
        message: EmailMessage,
    ) -> Result<EmailDelivery, ApplicationError> {
        let api_key = self.secrets.resolve(credential_secret_reference).await?;
        if message.idempotency_key.is_empty() || message.idempotency_key.len() > 256 {
            return Err(ApplicationError::Conflict {
                code: "invalid_email_idempotency_key",
                message: "The email idempotency key must contain between 1 and 256 bytes",
            });
        }
        let endpoint = self
            .api_base_url
            .join("emails")
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        let mut authorization =
            header::HeaderValue::from_str(&format!("Bearer {}", api_key.expose_secret()))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        authorization.set_sensitive(true);
        let response = self
            .client
            .post(endpoint)
            .header(header::AUTHORIZATION, authorization)
            .header("Idempotency-Key", &message.idempotency_key)
            .json(&SendRequest {
                from: &message.from,
                to: [&message.to],
                subject: &message.subject,
                text: &message.text,
                html: message.html.as_deref(),
            })
            .send()
            .await
            .map_err(|error| ApplicationError::Unavailable {
                service: "email",
                source: error.into(),
            })?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(ApplicationError::Unavailable {
                service: "email",
                source: anyhow::anyhow!("Resend returned HTTP {status}"),
            });
        }
        if !status.is_success() {
            return Err(ApplicationError::Conflict {
                code: "email_provider_rejected",
                message: "The email provider rejected the delivery request",
            });
        }
        let response: SendResponse =
            response
                .json()
                .await
                .map_err(|error| ApplicationError::Unavailable {
                    service: "email",
                    source: error.into(),
                })?;
        if response.id.is_empty() || response.id.len() > 255 {
            return Err(ApplicationError::Unavailable {
                service: "email",
                source: anyhow::anyhow!("Resend returned an invalid email identifier"),
            });
        }
        Ok(EmailDelivery {
            provider_message_id: response.id,
        })
    }
}
