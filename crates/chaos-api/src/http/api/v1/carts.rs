//! Cart lifecycle and checkout endpoints.

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, header},
    response::IntoResponse,
    routing::{get, post, put},
};
use chaos_core::{
    contracts::{CartDetail, CartLineItem, PaymentClientAction},
    payments::{CreateEmbeddedCheckoutInput, EmbeddedCheckoutResult},
    sales::{
        CheckoutAttributionInput, CreateCartInput, CreateStripeCheckoutInput, RemoveCartLineInput,
        SetCartLineInput, UtmTags,
    },
};
use chaos_domain::{catalog::ProductVariantId, sales::CartId};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, ShopperContext, invalid_value,
};

use super::wire::{CartStatus, MediaResponse, PaymentClientActionType, PaymentProvider};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/carts", post(create_cart))
        .route("/carts/{cart_id}", get(get_cart))
        .route("/carts/{cart_id}/lines/{product_variant_id}", put(set_cart_line).delete(remove_cart_line))
        .route("/carts/{cart_id}/checkout", post(create_embedded_checkout))
}

// ===== request contracts =====

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
struct CreateCartRequest {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetCartLineRequest {
    quantity: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateEmbeddedCheckoutRequest {
    return_url: String,
    payment_provider: PaymentProvider,
    #[serde(default)]
    attribution: Option<AttributionRequest>,
}

/// Ad-platform attribution captured at checkout and saved for the
/// server-side Purchase event. `source_url` and `utm` are not
/// platform-specific, so they sit alongside the per-platform `meta`
/// namespace.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttributionRequest {
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    utm: Option<UtmAttributionRequest>,
    #[serde(default)]
    meta: Option<MetaAttributionRequest>,
}

/// Standard `utm_*` campaign tags, minus the redundant `utm_` prefix since
/// they are already namespaced under `utm`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UtmAttributionRequest {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    campaign: Option<String>,
    #[serde(default)]
    term: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaAttributionRequest {
    #[serde(default)]
    fbc: Option<String>,
    #[serde(default)]
    fbp: Option<String>,
}

// ===== response contracts =====

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
    client_action: PaymentClientActionResponse,
}

#[derive(Serialize)]
struct PaymentClientActionResponse {
    r#type: PaymentClientActionType,
    public_key: String,
    client_token: String,
}

impl CartResponse {
    fn from_detail(cart: CartDetail) -> Self {
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

impl From<EmbeddedCheckoutResult> for EmbeddedCheckoutResponse {
    fn from(checkout: EmbeddedCheckoutResult) -> Self {
        Self {
            order_id: checkout.order_id.as_uuid(),
            order_number: checkout.order_number,
            client_action: checkout.client_action.into(),
        }
    }
}

impl From<PaymentClientAction> for PaymentClientActionResponse {
    fn from(value: PaymentClientAction) -> Self {
        Self {
            r#type: value.kind.into(),
            public_key: value.public_key.expose_secret().to_owned(),
            client_token: value.client_token.expose_secret().to_owned(),
        }
    }
}

// ===== request mapping =====

/// `client_ip_address`/`client_user_agent` come from this request itself
/// (`X-Real-IP` is set by `deploy/nginx` from the real client address,
/// behind Cloudflare's realip module), never from the request body — the
/// browser has no trustworthy way to report either.
fn attribution_input(
    attribution: Option<&AttributionRequest>,
    headers: &HeaderMap,
) -> Option<CheckoutAttributionInput> {
    let client_ip_address = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let client_user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let meta = attribution.and_then(|value| value.meta.as_ref());
    let utm = attribution.and_then(|value| value.utm.as_ref());
    Some(CheckoutAttributionInput {
        meta_fbc: meta.and_then(|meta| meta.fbc.clone()),
        meta_fbp: meta.and_then(|meta| meta.fbp.clone()),
        client_ip_address,
        client_user_agent,
        source_url: attribution.and_then(|value| value.source_url.clone()),
        utm: UtmTags {
            source: utm.and_then(|utm| utm.source.clone()),
            medium: utm.and_then(|utm| utm.medium.clone()),
            campaign: utm.and_then(|utm| utm.campaign.clone()),
            term: utm.and_then(|utm| utm.term.clone()),
            content: utm.and_then(|utm| utm.content.clone()),
        },
    })
}

// ===== POST /carts =====

async fn create_cart(
    State(state): State<ApiState>,
    ShopperContext(actor): ShopperContext,
    ApiJson(CreateCartRequest {}): ApiJson<CreateCartRequest>,
) -> Result<ApiResponse<CartResponse>, ApiError> {
    let cart = state
        .storefront_sales
        .create_cart(CreateCartInput { actor })
        .await?;
    Ok(ApiResponse::created(CartResponse::from_detail(cart)))
}

// ===== GET /carts/{cart_id} =====

async fn get_cart(
    State(state): State<ApiState>,
    ShopperContext(actor): ShopperContext,
    ApiPath(path): ApiPath<CartPath>,
) -> Result<ApiResponse<CartResponse>, ApiError> {
    let cart = state
        .storefront_sales
        .get_cart(&actor, CartId::from_uuid(path.cart_id))
        .await?;
    Ok(ApiResponse::ok(CartResponse::from_detail(cart)))
}

// ===== PUT /carts/{cart_id}/lines/{product_variant_id} =====

async fn set_cart_line(
    State(state): State<ApiState>,
    ShopperContext(actor): ShopperContext,
    ApiPath(path): ApiPath<CartLinePath>,
    ApiJson(request): ApiJson<SetCartLineRequest>,
) -> Result<ApiResponse<CartResponse>, ApiError> {
    let cart = state
        .storefront_sales
        .set_cart_line(SetCartLineInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
            quantity: request.quantity,
        })
        .await?;
    Ok(ApiResponse::ok(CartResponse::from_detail(cart)))
}

// ===== DELETE /carts/{cart_id}/lines/{product_variant_id} =====

async fn remove_cart_line(
    State(state): State<ApiState>,
    ShopperContext(actor): ShopperContext,
    ApiPath(path): ApiPath<CartLinePath>,
) -> Result<ApiResponse<CartResponse>, ApiError> {
    let cart = state
        .storefront_sales
        .remove_cart_line(RemoveCartLineInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
        })
        .await?;
    Ok(ApiResponse::ok(CartResponse::from_detail(cart)))
}

// ===== POST /carts/{cart_id}/checkout =====

async fn create_embedded_checkout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ShopperContext(actor): ShopperContext,
    ApiPath(path): ApiPath<CartPath>,
    ApiJson(request): ApiJson<CreateEmbeddedCheckoutRequest>,
) -> Result<impl IntoResponse, ApiError> {
    validate_return_url(&request.return_url)?;
    let now = state.clock.now();
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
            return_url: request.return_url.clone(),
            payment_provider: request.payment_provider.into(),
            now,
            idempotency_key,
            attribution: attribution_input(request.attribution.as_ref(), &headers),
        })
        .await?;
    let checkout = state
        .payment_service
        .create_embedded_checkout(CreateEmbeddedCheckoutInput {
            actor,
            order_id: draft.order_id,
            return_url: request.return_url,
            now,
        })
        .await?;
    Ok((
        sensitive_response_headers(),
        ApiResponse::created(EmbeddedCheckoutResponse::from(checkout)),
    ))
}

fn sensitive_response_headers() -> [(header::HeaderName, &'static str); 2] {
    [
        (header::CACHE_CONTROL, "private, no-store"),
        (header::REFERRER_POLICY, "no-referrer"),
    ]
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
                        "return_url": "https://shop.example.test/return",
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
