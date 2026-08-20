use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Serialize;

use chaos_application::ApplicationError;

use super::{ApiError, ApiResponse, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/webhooks/v1/notifications/{provider}",
            post(receive_webhook),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Serialize)]
struct WebhookReceiptData {
    accepted: bool,
}

async fn receive_webhook(
    State(state): State<ApiState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ApiResponse<WebhookReceiptData>, ApiError> {
    let message_id = required_header(&headers, "svix-id")?;
    let timestamp = required_header(&headers, "svix-timestamp")?;
    let signature = required_header(&headers, "svix-signature")?;
    let accepted = state
        .notification_webhooks
        .receive(
            &provider,
            message_id,
            timestamp,
            signature,
            &body,
            state.clock.now(),
        )
        .await?;
    Ok(ApiResponse::new(
        StatusCode::ACCEPTED,
        WebhookReceiptData { accepted },
    ))
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApplicationError::Unauthorized.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::{body::Body, http::Request};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use chaos_application::{
        notifications::NotificationWebhooks,
        ports::{
            EmailDelivery, EmailDeliveryFailure, EmailDeliveryJob, EmailDeliveryRepository,
            EmailWebhookVerifier, VerifiedEmailWebhook,
        },
    };
    use chaos_infrastructure::email::ResendWebhookVerifier;
    use hmac::{Hmac, KeyInit, Mac};
    use secrecy::SecretString;
    use sha2::Sha256;
    use time::OffsetDateTime;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::super::tests::test_state;
    use super::*;

    struct AcceptingRepository;

    #[async_trait]
    impl EmailDeliveryRepository for AcceptingRepository {
        async fn claim(&self, _limit: u16) -> Result<Vec<EmailDeliveryJob>, ApplicationError> {
            unreachable!()
        }

        async fn finish(
            &self,
            _delivery_id: Uuid,
            _attempts: u32,
            _result: Result<EmailDelivery, EmailDeliveryFailure>,
            _now: OffsetDateTime,
        ) -> Result<(), ApplicationError> {
            unreachable!()
        }

        async fn record_webhook(
            &self,
            _event: &VerifiedEmailWebhook,
        ) -> Result<bool, ApplicationError> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn resend_webhook_http_requires_and_verifies_svix_headers() {
        let key = b"0123456789abcdef0123456789abcdef";
        let secret = SecretString::from(format!("whsec_{}", STANDARD.encode(key)));
        let verifier =
            Arc::new(ResendWebhookVerifier::new(&secret).unwrap()) as Arc<dyn EmailWebhookVerifier>;
        let mut state = test_state();
        state.notification_webhooks = Arc::new(NotificationWebhooks::new(
            Arc::new(AcceptingRepository),
            vec![verifier],
        ));
        let app = super::super::router(state);
        let payload = r#"{"type":"email.delivered","data":{"email_id":"email_123"}}"#;
        let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();
        let message_id = "msg_http_123";
        let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
        mac.update(format!("{message_id}.{timestamp}.{payload}").as_bytes());
        let signature = format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()));

        let accepted = app
            .clone()
            .oneshot(
                Request::post("/webhooks/v1/notifications/resend")
                    .header("content-type", "application/json")
                    .header("svix-id", message_id)
                    .header("svix-timestamp", &timestamp)
                    .header("svix-signature", signature)
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);

        let missing_headers = app
            .oneshot(
                Request::post("/webhooks/v1/notifications/resend")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_headers.status(), StatusCode::UNAUTHORIZED);
    }
}
