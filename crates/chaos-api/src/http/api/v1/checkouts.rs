use axum::{Router, extract::State, http::HeaderMap, routing::post};
use chaos_core::{
    contracts::PaymentClientAction, payments::CreateEmbeddedCheckoutInput,
    sales::CreateStripeCheckoutInput,
};
use chaos_domain::{integration::PaymentProvider, sales::CartId};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiError, ApiJson, ApiPath, ApiResponse, ApiState, ShopperContext, invalid_value,
};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new().route("/carts/{cart_id}/checkout", post(create_embedded_checkout))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateEmbeddedCheckoutBody {
    /// Optional: Stripe Embedded Checkout collects the shopper's email
    /// directly when the channel does not already have one.
    #[serde(default)]
    email: Option<String>,
    return_url: String,
    payment_provider: String,
}

#[derive(Deserialize)]
struct CheckoutPath {
    cart_id: Uuid,
}

#[derive(Serialize)]
struct EmbeddedCheckoutData {
    order_number: String,
    source_cart_id: Uuid,
    client_action: PaymentClientActionData,
}

#[derive(Serialize)]
struct PaymentClientActionData {
    r#type: &'static str,
    public_key: String,
    client_token: String,
}

async fn create_embedded_checkout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ShopperContext(actor): ShopperContext,
    ApiPath(path): ApiPath<CheckoutPath>,
    ApiJson(body): ApiJson<CreateEmbeddedCheckoutBody>,
) -> Result<ApiResponse<EmbeddedCheckoutData>, ApiError> {
    validate_return_url(&body.return_url)?;
    let payment_provider = PaymentProvider::parse(&body.payment_provider).ok_or_else(|| {
        invalid_value(
            "payment_provider",
            "must be a supported payment provider such as stripe",
        )
    })?;
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
        .ok_or_else(|| invalid_value("Idempotency-Key", "must be a valid UUID"))?;
    let draft = state
        .storefront_sales
        .create_stripe_checkout(CreateStripeCheckoutInput {
            actor: actor.clone(),
            cart_id: CartId::from_uuid(path.cart_id),
            email: body.email.clone().filter(|value| !value.trim().is_empty()),
            return_url: body.return_url.clone(),
            payment_provider,
            now: state.clock.now(),
            idempotency_key,
        })
        .await?;
    let checkout = state
        .payment_service
        .create_embedded_checkout(CreateEmbeddedCheckoutInput {
            actor,
            order_id: draft.order_id,
            return_url: body.return_url,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(embedded_checkout_data(checkout)))
}

fn embedded_checkout_data(
    checkout: chaos_core::payments::EmbeddedCheckoutResult,
) -> EmbeddedCheckoutData {
    EmbeddedCheckoutData {
        order_number: checkout.order_number,
        source_cart_id: checkout.source_cart_id.as_uuid(),
        client_action: client_action_data(checkout.client_action),
    }
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

fn client_action_data(value: PaymentClientAction) -> PaymentClientActionData {
    PaymentClientActionData {
        r#type: value.kind,
        public_key: value.public_key.expose_secret().to_owned(),
        client_token: value.client_token.expose_secret().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use chaos_core::{contracts::PaymentClientAction, payments::EmbeddedCheckoutResult};
    use chaos_domain::sales::CartId;
    use secrecy::SecretString;
    use serde_json::json;
    use uuid::Uuid;

    use super::{client_action_data, embedded_checkout_data, validate_return_url};

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

    #[test]
    fn embedded_checkout_response_exposes_order_number_instead_of_internal_id() {
        let response = embedded_checkout_data(EmbeddedCheckoutResult {
            order_number: "W-20260830-7K4M9Q2D".into(),
            source_cart_id: CartId::from_uuid(Uuid::now_v7()),
            client_action: PaymentClientAction {
                kind: "mount_embedded_checkout",
                public_key: SecretString::from("pk_test_stripe"),
                client_token: SecretString::from("cs_test_secret"),
            },
        });
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["order_number"], "W-20260830-7K4M9Q2D");
        assert!(value.get("order_id").is_none());
    }
}
