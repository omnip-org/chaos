use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    payments::CreatePaymentAttemptInput,
    ports::{IdempotencyRequest, PaymentAttemptDetail, PaymentClientAction},
};
use chaos_domain::{payments::PaymentAttemptId, sales::OrderId};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::http::shared::pagination::idempotency_key;
use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, CheckoutShopper,
};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/store/v1/orders/{order_id}/payment-attempts",
            post(create_attempt),
        )
        .route(
            "/store/v1/payment-attempts/{payment_attempt_id}",
            get(get_attempt),
        )
        .route(
            "/store/v1/payment-attempts/{payment_attempt_id}/client-action",
            get(get_client_action),
        )
        .route(
            "/webhooks/v1/payments/{provider}/{provider_account_id}",
            post(receive_webhook),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Deserialize)]
struct OrderPath {
    order_id: Uuid,
}

#[derive(Deserialize)]
struct AttemptPath {
    payment_attempt_id: Uuid,
}

#[derive(Deserialize)]
struct WebhookPath {
    provider: String,
    provider_account_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateAttemptBody {
    provider: String,
    #[serde(default)]
    return_url: Option<String>,
}

#[derive(Serialize)]
struct PaymentAttemptData {
    id: Uuid,
    order_id: Uuid,
    provider: String,
    amount_minor: i64,
    currency: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct PaymentClientActionData {
    provider: String,
    r#type: &'static str,
    public_key: String,
    client_token: String,
}

#[derive(Serialize)]
struct WebhookReceiptData {
    accepted: bool,
}

async fn create_attempt(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<OrderPath>,
    ApiJson(body): ApiJson<CreateAttemptBody>,
) -> Result<ApiResponse<PaymentAttemptData>, ApiError> {
    validate_return_url(&body)?;
    let idempotency = body_request(&headers, "create_payment_attempt", &(path.order_id, &body))?;
    let attempt = state
        .payment_service
        .create_attempt(CreatePaymentAttemptInput {
            actor,
            order_id: OrderId::from_uuid(path.order_id),
            provider: body.provider,
            return_url: body.return_url,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(attempt_data(attempt)))
}

fn validate_return_url(body: &CreateAttemptBody) -> Result<(), ApiError> {
    if let Some(value) = &body.return_url {
        let url = url::Url::parse(value)
            .map_err(|_| invalid_value("return_url", "must be an absolute URL"))?;
        let secure = url.scheme() == "https";
        let loopback = url.scheme() == "http"
            && url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
        if !secure && !loopback {
            return Err(invalid_value(
                "return_url",
                "must use https, except for an http loopback URL in local development",
            ));
        }
    }
    if body.provider == "stripe_checkout" && body.return_url.is_none() {
        return Err(invalid_value(
            "return_url",
            "return_url is required for the stripe_checkout provider",
        ));
    }
    Ok(())
}

async fn get_attempt(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<AttemptPath>,
) -> Result<ApiResponse<PaymentAttemptData>, ApiError> {
    let attempt = state
        .payment_service
        .get_attempt(&actor, PaymentAttemptId::from_uuid(path.payment_attempt_id))
        .await?;
    Ok(ApiResponse::ok(attempt_data(attempt)))
}

async fn get_client_action(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<AttemptPath>,
) -> Result<Response, ApiError> {
    let action = state
        .payment_service
        .get_client_action(&actor, PaymentAttemptId::from_uuid(path.payment_attempt_id))
        .await?;
    let mut response = ApiResponse::ok(client_action_data(action)).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn receive_webhook(
    State(state): State<ApiState>,
    ApiPath(path): ApiPath<WebhookPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ApiResponse<WebhookReceiptData>, ApiError> {
    let signature = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApplicationError::Unauthorized)?;
    let accepted = state
        .payment_service
        .receive_webhook(
            &path.provider,
            path.provider_account_id,
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

fn client_action_data(value: PaymentClientAction) -> PaymentClientActionData {
    PaymentClientActionData {
        provider: value.provider,
        r#type: value.kind,
        public_key: value.public_key.expose_secret().to_owned(),
        client_token: value.client_token.expose_secret().to_owned(),
    }
}

fn body_request<T: Serialize>(
    headers: &HeaderMap,
    operation: &'static str,
    body: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(operation, body))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

fn attempt_data(value: PaymentAttemptDetail) -> PaymentAttemptData {
    PaymentAttemptData {
        id: value.id.as_uuid(),
        order_id: value.order_id.as_uuid(),
        provider: value.provider,
        amount_minor: value.amount_minor,
        currency: value.currency.as_str().into(),
        status: value.status.as_str(),
        provider_reference: value.provider_reference,
        failure_code: value.failure_code,
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    }
}

fn invalid_value(field: &'static str, reason: &'static str) -> ApiError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
    .into()
}

#[cfg(test)]
mod tests {
    use chaos_application::ports::PaymentClientAction;
    use secrecy::SecretString;
    use serde_json::json;

    use super::{CreateAttemptBody, client_action_data, validate_return_url};

    #[test]
    fn embedded_checkout_requires_a_secure_or_loopback_return_url() {
        assert!(
            validate_return_url(&CreateAttemptBody {
                provider: "stripe_checkout".into(),
                return_url: Some("https://shop.example.com/checkout/success".into()),
            })
            .is_ok()
        );
        assert!(
            validate_return_url(&CreateAttemptBody {
                provider: "stripe_checkout".into(),
                return_url: Some("http://127.0.0.1:4321/checkout/success".into()),
            })
            .is_ok()
        );
        assert!(
            validate_return_url(&CreateAttemptBody {
                provider: "stripe_checkout".into(),
                return_url: Some("http://shop.example.com/checkout/success".into()),
            })
            .is_err()
        );
        assert!(
            validate_return_url(&CreateAttemptBody {
                provider: "stripe_checkout".into(),
                return_url: None,
            })
            .is_err()
        );
    }

    #[test]
    fn embedded_checkout_client_action_has_no_connect_account_reference() {
        let action = client_action_data(PaymentClientAction {
            provider: "stripe_checkout".into(),
            kind: "mount_embedded_checkout",
            public_key: SecretString::from("pk_test_stripe"),
            client_token: SecretString::from("cs_test_secret"),
        });

        assert_eq!(
            serde_json::to_value(action).unwrap(),
            json!({
                "provider": "stripe_checkout",
                "type": "mount_embedded_checkout",
                "public_key": "pk_test_stripe",
                "client_token": "cs_test_secret",
            })
        );
    }
}
