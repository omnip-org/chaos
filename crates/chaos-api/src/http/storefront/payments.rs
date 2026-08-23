use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use chaos_application::{
    ApplicationError,
    payments::CreatePaymentAttemptInput,
    ports::{IdempotencyRequest, PaymentClientAction},
    sales::CreateStripeCheckoutInput,
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::http::shared::pagination::idempotency_key;
use crate::http::{ApiError, ApiJson, ApiPath, ApiResponse, ApiState, PaymentShopper};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/store/v1/carts/{cart_id}/embedded-checkout",
            post(create_embedded_checkout),
        )
        .route(
            "/store/v1/webhooks/stripe/{stripe_account_id}",
            post(receive_webhook),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Deserialize)]
struct CartPath {
    cart_id: Uuid,
}

#[derive(Deserialize)]
struct WebhookPath {
    stripe_account_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateEmbeddedCheckoutBody {
    email: String,
    return_url: String,
}

#[derive(Serialize)]
struct EmbeddedCheckoutData {
    order_id: Uuid,
    payment_attempt_id: Uuid,
    client_action: PaymentClientActionData,
}

#[derive(Serialize)]
struct PaymentClientActionData {
    r#type: &'static str,
    public_key: String,
    client_token: String,
}

#[derive(Serialize)]
struct WebhookReceiptData {
    accepted: bool,
}

async fn create_embedded_checkout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    PaymentShopper(actor): PaymentShopper,
    ApiPath(path): ApiPath<CartPath>,
    ApiJson(body): ApiJson<CreateEmbeddedCheckoutBody>,
) -> Result<ApiResponse<EmbeddedCheckoutData>, ApiError> {
    validate_return_url(&body.return_url)?;
    let idempotency = body_request(&headers, "create_stripe_checkout", &(path.cart_id, &body))?;
    let draft = state
        .storefront_sales
        .create_stripe_checkout(CreateStripeCheckoutInput {
            actor: actor.clone(),
            cart_id: chaos_domain::sales::CartId::from_uuid(path.cart_id),
            email: body.email.clone(),
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    let mut return_url = url::Url::parse(&body.return_url)
        .map_err(|_| invalid_value("return_url", "must be an absolute URL"))?;
    return_url
        .query_pairs_mut()
        .append_pair("order_id", &draft.order_id.as_uuid().to_string());
    let checkout = state
        .payment_service
        .create_embedded_checkout(CreatePaymentAttemptInput {
            actor: actor.clone(),
            order_id: draft.order_id,
            return_url: Some(return_url.to_string()),
            now: state.clock.now(),
            idempotency: body_request(
                &headers,
                "create_embedded_checkout_payment_attempt",
                &(draft.order_id.as_uuid(), &body),
            )?,
        })
        .await?;
    Ok(ApiResponse::created(EmbeddedCheckoutData {
        order_id: draft.order_id.as_uuid(),
        payment_attempt_id: checkout.attempt.id.as_uuid(),
        client_action: client_action_data(checkout.client_action),
    }))
}

fn validate_return_url(value: &str) -> Result<(), ApiError> {
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
    Ok(())
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
        .receive_webhook(path.stripe_account_id, signature, &body, state.clock.now())
        .await?;
    Ok(ApiResponse::new(
        StatusCode::ACCEPTED,
        WebhookReceiptData { accepted },
    ))
}

fn client_action_data(value: PaymentClientAction) -> PaymentClientActionData {
    PaymentClientActionData {
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

    use super::{client_action_data, validate_return_url};

    #[test]
    fn embedded_checkout_requires_a_secure_or_loopback_return_url() {
        assert!(validate_return_url("https://shop.example.com/checkout/success").is_ok());
        assert!(validate_return_url("http://127.0.0.1:4321/checkout/success").is_ok());
        assert!(validate_return_url("http://shop.example.com/checkout/success").is_err());
    }

    #[test]
    fn embedded_checkout_client_action_has_no_connect_account_reference() {
        let action = client_action_data(PaymentClientAction {
            kind: "mount_embedded_checkout",
            public_key: SecretString::from("pk_test_stripe"),
            client_token: SecretString::from("cs_test_secret"),
        });

        assert_eq!(
            serde_json::to_value(action).unwrap(),
            json!({
                "type": "mount_embedded_checkout",
                "public_key": "pk_test_stripe",
                "client_token": "cs_test_secret",
            })
        );
    }
}
