use axum::{
    Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post, put},
};
use chaos_core::{
    ApplicationError,
    contracts::{
        CartDetail, CartLineItem, PaymentClientAction, StorefrontMediaAsset, StorefrontMediaScope,
    },
    payments::CreatePaymentAttemptInput,
    sales::CreateStripeCheckoutInput,
    sales::{CreateCartInput, RemoveCartLineInput, SetCartLineInput},
};
use chaos_domain::{catalog::ProductVariantId, integration::PaymentProvider, sales::CartId};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, CartShopper, PaymentShopper,
};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/carts", post(create_cart))
        .route("/carts/{cart_id}", get(get_cart))
        .route("/carts/{cart_id}/lines/{product_variant_id}", put(set_cart_line).delete(remove_cart_line))
        .route("/carts/{cart_id}/checkout", post(create_embedded_checkout))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateCartBody {}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetCartLineBody {
    quantity: u32,
}

#[derive(Deserialize)]
struct CartPath {
    cart_id: Uuid,
}

#[derive(Deserialize)]
struct CartLinePath {
    cart_id: Uuid,
    product_variant_id: Uuid,
}

#[derive(Serialize)]
struct CartLineData {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    media: Vec<CartMediaData>,
}

#[derive(Serialize)]
struct CartMediaData {
    id: Uuid,
    scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    option_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    option_value_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_variant_id: Option<Uuid>,
    media_type: String,
    kind: &'static str,
    alt_text: String,
    position: u16,
    url: String,
}

#[derive(Serialize)]
struct CartData {
    id: Uuid,
    price_list_id: Uuid,
    currency: String,
    status: &'static str,
    version: u64,
    lines: Vec<CartLineData>,
    subtotal_amount_minor: i64,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

async fn create_cart(
    State(state): State<ApiState>,
    CartShopper(actor): CartShopper,
    ApiJson(CreateCartBody {}): ApiJson<CreateCartBody>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let cart = state
        .storefront_sales
        .create_cart(CreateCartInput { actor })
        .await?;
    Ok(ApiResponse::created(cart_data(cart)?))
}

async fn get_cart(
    State(state): State<ApiState>,
    CartShopper(actor): CartShopper,
    ApiPath(path): ApiPath<CartPath>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let cart = state
        .storefront_sales
        .get_cart(&actor, CartId::from_uuid(path.cart_id))
        .await?;
    Ok(ApiResponse::ok(cart_data(cart)?))
}

async fn set_cart_line(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartShopper(actor): CartShopper,
    ApiPath(path): ApiPath<CartLinePath>,
    ApiJson(body): ApiJson<SetCartLineBody>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let expected_version = expected_cart_version(&headers)?;
    let cart = state
        .storefront_sales
        .set_cart_line(SetCartLineInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
            quantity: body.quantity,
            expected_version,
        })
        .await?;
    Ok(ApiResponse::ok(cart_data(cart)?))
}

async fn remove_cart_line(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartShopper(actor): CartShopper,
    ApiPath(path): ApiPath<CartLinePath>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let expected_version = expected_cart_version(&headers)?;
    let cart = state
        .storefront_sales
        .remove_cart_line(RemoveCartLineInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
            expected_version,
        })
        .await?;
    Ok(ApiResponse::ok(cart_data(cart)?))
}

fn cart_data(cart: CartDetail) -> Result<CartData, ApplicationError> {
    Ok(CartData {
        id: cart.id.as_uuid(),
        price_list_id: cart.price_list_id.as_uuid(),
        currency: cart.currency.as_str().to_owned(),
        status: cart.status.as_str(),
        version: cart.version,
        lines: cart.lines.into_iter().map(cart_line_data).collect(),
        subtotal_amount_minor: cart.subtotal_amount_minor,
        created_at: cart.created_at.into(),
        updated_at: cart.updated_at.into(),
    })
}

fn cart_line_data(line: CartLineItem) -> CartLineData {
    CartLineData {
        product_id: line.product_id.as_uuid(),
        product_variant_id: line.product_variant_id.as_uuid(),
        product_title: line.product_title,
        variant_title: line.variant_title,
        sku: line.sku,
        track_inventory: line.track_inventory,
        quantity: line.quantity,
        unit_price_amount_minor: line.unit_price_amount_minor,
        subtotal_amount_minor: line.subtotal_amount_minor,
        media: line.media.into_iter().map(cart_media_data).collect(),
    }
}

fn cart_media_data(media: StorefrontMediaAsset) -> CartMediaData {
    let (scope, option_id, option_value_id, product_variant_id) = match media.scope {
        StorefrontMediaScope::Product => ("product", None, None, None),
        StorefrontMediaScope::OptionValue {
            option_id,
            option_value_id,
        } => (
            "option_value",
            Some(option_id.as_uuid()),
            Some(option_value_id.as_uuid()),
            None,
        ),
        StorefrontMediaScope::Variant { product_variant_id } => {
            ("variant", None, None, Some(product_variant_id.as_uuid()))
        }
    };
    CartMediaData {
        id: media.id.as_uuid(),
        scope,
        option_id,
        option_value_id,
        product_variant_id,
        media_type: media.media_type,
        kind: media.kind.as_str(),
        alt_text: media.alt_text,
        position: media.position,
        url: media.url,
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateEmbeddedCheckoutBody {
    /// Optional: Stripe Embedded Checkout collects the shopper's email
    /// directly when the storefront does not already have one.
    #[serde(default)]
    email: Option<String>,
    return_url: String,
    payment_provider: String,
}

#[derive(Serialize)]
struct EmbeddedCheckoutData {
    order_id: Uuid,
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
    PaymentShopper(actor): PaymentShopper,
    ApiPath(path): ApiPath<CartPath>,
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
            payment_provider,
            now: state.clock.now(),
            idempotency_key,
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
            provider: payment_provider.as_str().to_owned(),
            return_url: Some(return_url.to_string()),
            now: state.clock.now(),
            idempotency_key,
        })
        .await?;
    Ok(ApiResponse::created(EmbeddedCheckoutData {
        order_id: draft.order_id.as_uuid(),
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

fn client_action_data(value: PaymentClientAction) -> PaymentClientActionData {
    PaymentClientActionData {
        r#type: value.kind,
        public_key: value.public_key.expose_secret().to_owned(),
        client_token: value.client_token.expose_secret().to_owned(),
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

fn expected_cart_version(headers: &HeaderMap) -> Result<u64, ApiError> {
    let value = headers
        .get("if-match")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().trim_matches('"'))
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| invalid_value("If-Match", "must contain the Cart version"))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use chaos_core::contracts::PaymentClientAction;
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
