//! Cart lifecycle and checkout endpoints.

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, header},
    response::IntoResponse,
    routing::{get, post, put},
};
use chaos_core::{
    contracts::{CartDetail, CartLineItem},
    payments::{CreateEmbeddedCheckoutInput, EmbeddedCheckoutResult},
    sales::CreateCheckoutInput,
};
use chaos_domain::{catalog::ProductVariantId, sales::CartId};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, PrivateApiResponse,
    ShopperContext, invalid_value,
};

use super::{
    attribution::{CheckoutAttributionRequest, checkout_attribution_input},
    wire::{CartStatus, MediaResponse, PaymentClientActionResponse, PaymentProvider},
};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/carts", get(get_active_cart).post(create_cart))
        .route("/carts/{cart_id}", get(get_cart))
        .route(
            "/carts/{cart_id}/lines/{product_variant_id}",
            put(set_cart_line).delete(remove_cart_line),
        )
        .route("/carts/{cart_id}/checkout", post(create_embedded_checkout))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetCartLineRequest {
    quantity: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateEmbeddedCheckoutRequest {
    payment_provider: PaymentProvider,
    #[serde(default)]
    attribution: Option<CheckoutAttributionRequest>,
}

#[derive(Serialize)]
struct CartResponse {
    id: Uuid,
    currency: String,
    status: CartStatus,
    lines: Vec<CartLineResponse>,
    subtotal_amount_minor: i64,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct CartLineResponse {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    media: Vec<MediaResponse>,
}

#[derive(Serialize)]
struct EmbeddedCheckoutResponse {
    order_id: Uuid,
    order_number: String,
    checkout_token: String,
    client_action: PaymentClientActionResponse,
}

impl From<CartDetail> for CartResponse {
    fn from(cart: CartDetail) -> Self {
        Self {
            id: cart.id.as_uuid(),
            currency: cart.currency.as_str().to_owned(),
            status: cart.status.into(),
            lines: cart.lines.into_iter().map(Into::into).collect(),
            subtotal_amount_minor: cart.subtotal_amount_minor,
            created_at: cart.created_at.into(),
            updated_at: cart.updated_at.into(),
        }
    }
}

impl From<CartLineItem> for CartLineResponse {
    fn from(line: CartLineItem) -> Self {
        Self {
            product_id: line.product_id.as_uuid(),
            product_variant_id: line.product_variant_id.as_uuid(),
            product_title: line.product_title,
            variant_title: line.variant_title,
            sku: line.sku,
            quantity: line.quantity,
            unit_price_amount_minor: line.unit_price_amount_minor,
            subtotal_amount_minor: line.subtotal_amount_minor,
            media: line.media.into_iter().map(Into::into).collect(),
        }
    }
}

impl EmbeddedCheckoutResponse {
    fn new(checkout: EmbeddedCheckoutResult, checkout_token: String) -> Self {
        Self {
            order_id: checkout.order_id.as_uuid(),
            order_number: checkout.order_number,
            checkout_token,
            client_action: checkout.client_action.into(),
        }
    }
}

async fn create_cart(
    State(state): State<ApiState>,
    ShopperContext(shopper): ShopperContext,
) -> Result<PrivateApiResponse<CartResponse>, ApiError> {
    let cart = state.storefront_sales.create_cart(shopper).await?;
    Ok(ApiResponse::created(cart.into()).private())
}

async fn get_active_cart(
    State(state): State<ApiState>,
    ShopperContext(shopper): ShopperContext,
) -> Result<PrivateApiResponse<CartResponse>, ApiError> {
    let cart = state.storefront_sales.get_active_cart(&shopper).await?;
    Ok(ApiResponse::ok(cart.into()).private())
}

async fn get_cart(
    State(state): State<ApiState>,
    ShopperContext(shopper): ShopperContext,
    ApiPath(path): ApiPath<CartPath>,
) -> Result<PrivateApiResponse<CartResponse>, ApiError> {
    let cart = state
        .storefront_sales
        .get_cart(&shopper, CartId::from_uuid(path.cart_id))
        .await?;
    Ok(ApiResponse::ok(cart.into()).private())
}

async fn set_cart_line(
    State(state): State<ApiState>,
    ShopperContext(shopper): ShopperContext,
    ApiPath(path): ApiPath<CartLinePath>,
    ApiJson(request): ApiJson<SetCartLineRequest>,
) -> Result<PrivateApiResponse<CartResponse>, ApiError> {
    let cart = state
        .storefront_sales
        .set_cart_line(
            shopper,
            CartId::from_uuid(path.cart_id),
            ProductVariantId::from_uuid(path.product_variant_id),
            request.quantity,
        )
        .await?;
    Ok(ApiResponse::ok(cart.into()).private())
}

async fn remove_cart_line(
    State(state): State<ApiState>,
    ShopperContext(shopper): ShopperContext,
    ApiPath(path): ApiPath<CartLinePath>,
) -> Result<PrivateApiResponse<CartResponse>, ApiError> {
    let cart = state
        .storefront_sales
        .remove_cart_line(
            shopper,
            CartId::from_uuid(path.cart_id),
            ProductVariantId::from_uuid(path.product_variant_id),
        )
        .await?;
    Ok(ApiResponse::ok(cart.into()).private())
}

async fn create_embedded_checkout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ShopperContext(shopper): ShopperContext,
    ApiPath(path): ApiPath<CartPath>,
    ApiJson(request): ApiJson<CreateEmbeddedCheckoutRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let now = state.clock.now();
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
        .ok_or_else(|| invalid_value("Idempotency-Key", "must be a valid UUID"))?;
    let order_id = state
        .storefront_sales
        .create_checkout(CreateCheckoutInput {
            shopper: shopper.clone(),
            cart_id: CartId::from_uuid(path.cart_id),
            payment_provider: request.payment_provider.into(),
            now,
            idempotency_key,
            attribution: checkout_attribution_input(request.attribution.as_ref(), &headers),
        })
        .await?;
    let checkout_token = state
        .checkout_credentials
        .issue(&shopper.machine, order_id, now)?;
    let checkout = state
        .payment_service
        .create_embedded_checkout(CreateEmbeddedCheckoutInput {
            actor: shopper,
            order_id,
            now,
        })
        .await?;
    Ok((
        [(header::REFERRER_POLICY, "no-referrer")],
        ApiResponse::created(EmbeddedCheckoutResponse::new(
            checkout,
            checkout_token.expose_secret().to_owned(),
        ))
        .private(),
    ))
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
        routing::post,
    };
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    async fn decode_cart_line_request(
        ApiJson(_request): ApiJson<SetCartLineRequest>,
    ) -> StatusCode {
        StatusCode::OK
    }

    async fn decode_checkout_request(
        ApiJson(_request): ApiJson<CreateEmbeddedCheckoutRequest>,
    ) -> StatusCode {
        StatusCode::OK
    }

    #[tokio::test]
    async fn payment_provider_is_validated_during_json_deserialization() {
        let app = Router::new().route("/", post(decode_checkout_request));
        let request = |provider: &str| {
            Request::post("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "payment_provider": provider,
                    })
                    .to_string(),
                ))
                .unwrap()
        };

        let accepted = app.clone().oneshot(request("stripe")).await.unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);

        let rejected = app.oneshot(request("unknown")).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(rejected.into_body(), 2048).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(json["error"]["code"], "invalid_json");
    }

    #[tokio::test]
    async fn cart_line_request_does_not_accept_attribution() {
        let app = Router::new().route("/", post(decode_cart_line_request));
        let accepted = app
            .clone()
            .oneshot(
                Request::post("/")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"quantity":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);

        let rejected = app
            .oneshot(
                Request::post("/")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"quantity":1,"attribution":{"source_url":"https://shop.example/product"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    }
}
