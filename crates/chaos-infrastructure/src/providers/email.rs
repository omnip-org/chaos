use std::sync::Arc;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chaos_application::{
    ApplicationError,
    ports::{
        EmailDelivery, EmailMessage, EmailProvider, EmailWebhookVerifier,
        NotificationSecretResolver, VerifiedEmailWebhook,
    },
};
use chaos_domain::notifications::NotificationSecretReference;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::{Client, StatusCode, Url, header};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use time::{Duration, OffsetDateTime};

#[derive(Clone)]
pub struct ResendEmailProvider {
    client: Client,
    api_base_url: Url,
    secrets: Arc<dyn NotificationSecretResolver>,
}

impl ResendEmailProvider {
    pub fn new(
        api_base_url: Url,
        secrets: Arc<dyn NotificationSecretResolver>,
        timeout: std::time::Duration,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            api_base_url.scheme() == "https" || api_base_url.host_str() == Some("localhost"),
            "RESEND_API_BASE_URL must use HTTPS except on localhost"
        );
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder().timeout(timeout).build()?;
        Ok(Self {
            client,
            api_base_url,
            secrets,
        })
    }
}

#[derive(Serialize)]
struct ResendSendRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<&'a str>,
}

#[derive(Deserialize)]
struct ResendSendResponse {
    id: String,
}

#[derive(Clone)]
pub struct ResendWebhookVerifier {
    secrets: Arc<dyn NotificationSecretResolver>,
}

impl ResendWebhookVerifier {
    pub fn new(secrets: Arc<dyn NotificationSecretResolver>) -> Self {
        Self { secrets }
    }
}

#[derive(Deserialize)]
struct ResendWebhookPayload {
    #[serde(rename = "type")]
    event_type: String,
    data: ResendWebhookData,
}

#[derive(Deserialize)]
struct ResendWebhookData {
    email_id: String,
}

#[async_trait]
impl EmailWebhookVerifier for ResendWebhookVerifier {
    fn name(&self) -> &'static str {
        "resend"
    }

    async fn verify(
        &self,
        webhook_secret_reference: &NotificationSecretReference,
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
        let signing_key = STANDARD
            .decode(encoded)
            .map_err(|_| ApplicationError::Unauthorized)?;
        if signing_key.len() < 16 {
            return Err(ApplicationError::Unauthorized);
        }
        if payload.len() > 64 * 1024 || message_id.is_empty() || message_id.len() > 255 {
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
        let verified = signature.split_whitespace().any(|candidate| {
            let Some(encoded) = candidate.strip_prefix("v1,") else {
                return false;
            };
            let Ok(expected) = STANDARD.decode(encoded) else {
                return false;
            };
            let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&signing_key) else {
                return false;
            };
            mac.update(&signed);
            mac.verify_slice(&expected).is_ok()
        });
        if !verified {
            return Err(ApplicationError::Unauthorized);
        }
        let parsed: ResendWebhookPayload =
            serde_json::from_slice(payload).map_err(|error| ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "body",
                    reason: format!("must be a valid Resend webhook: {error}"),
                }],
            })?;
        if !matches!(
            parsed.event_type.as_str(),
            "email.sent"
                | "email.delivered"
                | "email.delivery_delayed"
                | "email.bounced"
                | "email.complained"
                | "email.suppressed"
        ) || parsed.data.email_id.is_empty()
            || parsed.data.email_id.len() > 255
        {
            return Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "body",
                    reason: "must contain a supported email event and email identifier".into(),
                }],
            });
        }
        let payload = serde_json::from_slice(payload)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        Ok(VerifiedEmailWebhook {
            provider_event_id: message_id.to_owned(),
            provider_message_id: parsed.data.email_id,
            provider_event_type: parsed.event_type,
            payload,
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
        credential: &NotificationSecretReference,
        message: EmailMessage,
    ) -> Result<EmailDelivery, ApplicationError> {
        let api_key = self.secrets.resolve(credential).await?;
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
        let payload = ResendSendRequest {
            from: &message.from,
            to: [&message.to],
            subject: &message.subject,
            text: &message.text,
            html: message.html.as_deref(),
        };
        let mut authorization =
            header::HeaderValue::from_str(&format!("Bearer {}", api_key.expose_secret()))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?;
        authorization.set_sensitive(true);
        let response = self
            .client
            .post(endpoint)
            .header(header::AUTHORIZATION, authorization)
            .header("Idempotency-Key", &message.idempotency_key)
            .json(&payload)
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
        let response: ResendSendResponse =
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use secrecy::SecretString;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use super::*;

    struct StaticSecrets(SecretString);

    #[async_trait]
    impl NotificationSecretResolver for StaticSecrets {
        async fn resolve(
            &self,
            _reference: &NotificationSecretReference,
        ) -> Result<SecretString, ApplicationError> {
            Ok(self.0.clone())
        }
    }

    fn reference() -> NotificationSecretReference {
        NotificationSecretReference::new("enc://test-secret").expect("reference")
    }

    #[derive(Clone, Default)]
    struct RecordedRequest {
        authorization: String,
        idempotency_key: String,
        body: Value,
    }

    async fn send_email(
        State(recorded): State<Arc<Mutex<RecordedRequest>>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, Json<Value>) {
        let mut recorded = recorded.lock().expect("request lock");
        recorded.authorization = headers[header::AUTHORIZATION]
            .to_str()
            .expect("authorization header")
            .to_owned();
        recorded.idempotency_key = headers["idempotency-key"]
            .to_str()
            .expect("idempotency header")
            .to_owned();
        recorded.body = serde_json::from_slice(&body).expect("JSON request");
        (StatusCode::OK, Json(json!({ "id": "email_123" })))
    }

    async fn rate_limited() -> (StatusCode, Json<Value>) {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "message": "rate limited" })),
        )
    }

    async fn rejected() -> (StatusCode, Json<Value>) {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "message": "invalid recipient" })),
        )
    }

    async fn provider_for(handler: Router) -> (ResendEmailProvider, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, handler)
                .await
                .expect("email test server");
        });
        let provider = ResendEmailProvider::new(
            Url::parse(&format!("http://localhost:{}/", address.port())).expect("URL"),
            Arc::new(StaticSecrets(SecretString::from("re_test_secret"))),
            std::time::Duration::from_secs(1),
        )
        .expect("provider");
        (provider, server)
    }

    fn message() -> EmailMessage {
        EmailMessage {
            from: "Chaos <hello@example.com>".into(),
            to: "shopper@example.com".into(),
            subject: "Order confirmed".into(),
            text: "Thank you".into(),
            html: Some("<p>Thank you</p>".into()),
            idempotency_key: "notification-019123".into(),
        }
    }

    #[tokio::test]
    async fn resend_sends_provider_neutral_message_with_stable_idempotency() {
        let recorded = Arc::new(Mutex::new(RecordedRequest::default()));
        let (provider, server) = provider_for(
            Router::new()
                .route("/emails", post(send_email))
                .with_state(recorded.clone()),
        )
        .await;

        let delivery = provider
            .send(&reference(), message())
            .await
            .expect("delivery");

        assert_eq!(delivery.provider_message_id, "email_123");
        let recorded = recorded.lock().expect("request lock").clone();
        assert_eq!(recorded.authorization, "Bearer re_test_secret");
        assert_eq!(recorded.idempotency_key, "notification-019123");
        assert_eq!(recorded.body["to"], json!(["shopper@example.com"]));
        assert_eq!(recorded.body["html"], "<p>Thank you</p>");
        server.abort();
    }

    #[tokio::test]
    async fn resend_classifies_rate_limits_as_retryable() {
        let (provider, server) =
            provider_for(Router::new().route("/emails", post(rate_limited))).await;

        let error = provider
            .send(&reference(), message())
            .await
            .expect_err("rate limit");

        assert!(matches!(error, ApplicationError::Unavailable { .. }));
        server.abort();
    }

    #[tokio::test]
    async fn resend_classifies_invalid_requests_as_permanent() {
        let (provider, server) = provider_for(Router::new().route("/emails", post(rejected))).await;

        let error = provider
            .send(&reference(), message())
            .await
            .expect_err("rejection");

        assert!(matches!(
            error,
            ApplicationError::Conflict {
                code: "email_provider_rejected",
                ..
            }
        ));
        server.abort();
    }

    #[tokio::test]
    async fn resend_webhook_verifies_raw_body_signature_and_timestamp() {
        let key = b"0123456789abcdef0123456789abcdef";
        let secret = SecretString::from(format!("whsec_{}", STANDARD.encode(key)));
        let verifier = ResendWebhookVerifier::new(Arc::new(StaticSecrets(secret)));
        let received_at = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("time");
        let timestamp = received_at.unix_timestamp().to_string();
        let message_id = "msg_019123";
        let payload = br#"{"type":"email.delivered","data":{"email_id":"email_123"}}"#;
        let signed = format!(
            "{message_id}.{timestamp}.{}",
            std::str::from_utf8(payload).expect("payload")
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC");
        mac.update(signed.as_bytes());
        let signature = format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()));

        let event = verifier
            .verify(
                &reference(),
                message_id,
                &timestamp,
                &signature,
                payload,
                received_at,
            )
            .await
            .expect("verified webhook");

        assert_eq!(event.provider_event_id, message_id);
        assert_eq!(event.provider_message_id, "email_123");
        assert_eq!(event.provider_event_type, "email.delivered");
        assert!(
            verifier
                .verify(
                    &reference(),
                    message_id,
                    &timestamp,
                    "v1,AAAAAAAA",
                    payload,
                    received_at,
                )
                .await
                .is_err()
        );
        assert!(
            verifier
                .verify(
                    &reference(),
                    message_id,
                    &timestamp,
                    &signature,
                    payload,
                    received_at + Duration::minutes(6),
                )
                .await
                .is_err()
        );
    }
}
