use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{EmailDelivery, EmailMessage, EmailProvider},
};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
};
use reqwest::{Client, StatusCode, Url, header};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct SmtpEmailProvider {
    mailer: AsyncSmtpTransport<Tokio1Executor>,
}

impl SmtpEmailProvider {
    pub fn new(smtp_url: &str) -> anyhow::Result<Self> {
        let mailer = AsyncSmtpTransport::<Tokio1Executor>::from_url(smtp_url)
            .map_err(|error| anyhow::anyhow!("invalid SMTP_URL: {error}"))?
            .build();
        Ok(Self { mailer })
    }
}

#[async_trait]
impl EmailProvider for SmtpEmailProvider {
    fn name(&self) -> &'static str {
        "smtp"
    }

    async fn send(&self, message: EmailMessage) -> Result<EmailDelivery, ApplicationError> {
        let mut builder = Message::builder()
            .from(parse_mailbox(&message.from)?)
            .to(parse_mailbox(&message.to)?)
            .subject(message.subject);
        let email = if let Some(html) = message.html {
            builder
                .multipart(lettre::message::MultiPart::alternative_plain_html(
                    message.text,
                    html,
                ))
                .map_err(unexpected_email_error)?
        } else {
            builder = builder.header(ContentType::TEXT_PLAIN);
            builder.body(message.text).map_err(unexpected_email_error)?
        };
        let response =
            self.mailer
                .send(email)
                .await
                .map_err(|error| ApplicationError::Unavailable {
                    service: "email",
                    source: error.into(),
                })?;
        Ok(EmailDelivery {
            provider_message_id: response.message().next().unwrap_or("accepted").to_owned(),
        })
    }
}

#[derive(Clone)]
pub struct ResendEmailProvider {
    client: Client,
    api_base_url: Url,
    api_key: SecretString,
}

impl ResendEmailProvider {
    pub fn new(
        api_base_url: Url,
        api_key: SecretString,
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
            api_key,
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

#[async_trait]
impl EmailProvider for ResendEmailProvider {
    fn name(&self) -> &'static str {
        "resend"
    }

    async fn send(&self, message: EmailMessage) -> Result<EmailDelivery, ApplicationError> {
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
            header::HeaderValue::from_str(&format!("Bearer {}", self.api_key.expose_secret()))
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

fn parse_mailbox(value: &str) -> Result<Mailbox, ApplicationError> {
    value
        .parse::<Mailbox>()
        .map_err(|error| ApplicationError::Unexpected(anyhow::anyhow!(error)))
}

fn unexpected_email_error(error: lettre::error::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
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
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use super::*;

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
            SecretString::from("re_test_secret"),
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

        let delivery = provider.send(message()).await.expect("delivery");

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

        let error = provider.send(message()).await.expect_err("rate limit");

        assert!(matches!(error, ApplicationError::Unavailable { .. }));
        server.abort();
    }

    #[tokio::test]
    async fn resend_classifies_invalid_requests_as_permanent() {
        let (provider, server) = provider_for(Router::new().route("/emails", post(rejected))).await;

        let error = provider.send(message()).await.expect_err("rejection");

        assert!(matches!(
            error,
            ApplicationError::Conflict {
                code: "email_provider_rejected",
                ..
            }
        ));
        server.abort();
    }
}
