//! Cart lifecycle and embedded checkout endpoints.

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
    payments::CreateEmbeddedCheckoutInput,
    sales::{
        CheckoutAttributionInput, CreateCartInput, CreateStripeCheckoutInput, RemoveCartLineInput,
        SetCartLineInput, UtmTags,
    },
};
use chaos_domain::{catalog::ProductVariantId, integration::PaymentProvider, sales::CartId};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, ShopperContext, invalid_value,
};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/carts", post(create_cart::handler))
        .route("/carts/{cart_id}", get(get_cart::handler))
        .route("/carts/{cart_id}/lines/{product_variant_id}", put(set_cart_line::handler).delete(remove_cart_line::handler))
        .route("/carts/{cart_id}/checkout", post(create_embedded_checkout::handler))
}

// ===== shared wire types & mappers =====

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
struct CartData {
    id: Uuid,
    currency: String,
    status: &'static str,
    lines: Vec<CartLineData>,
    subtotal_amount_minor: i64,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
    /// Server-minted Meta CAPI `AddToCart` event id, present only on the
    /// response to a line mutation that raised the quantity. The browser
    /// SDK reuses it for the Pixel's own AddToCart so Meta deduplicates.
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<Uuid>,
}

#[derive(Serialize)]
struct CartLineData {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
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

fn cart_data(cart: CartDetail, event_id: Option<Uuid>) -> Result<CartData, ApplicationError> {
    Ok(CartData {
        id: cart.id.as_uuid(),
        currency: cart.currency.as_str().to_owned(),
        status: cart.status.as_str(),
        lines: cart.lines.into_iter().map(cart_line_data).collect(),
        subtotal_amount_minor: cart.subtotal_amount_minor,
        created_at: cart.created_at.into(),
        updated_at: cart.updated_at.into(),
        event_id,
    })
}

fn cart_line_data(line: CartLineItem) -> CartLineData {
    CartLineData {
        product_id: line.product_id.as_uuid(),
        product_variant_id: line.product_variant_id.as_uuid(),
        product_title: line.product_title,
        variant_title: line.variant_title,
        sku: line.sku,
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

/// Ad-platform attribution the browser read off its own cookies/URL, shared
/// by the checkout handler (InitiateCheckout) and the line-mutation handler
/// (AddToCart). `source_url` and `utm` are not platform-specific, so they
/// sit alongside the per-platform `meta` namespace.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttributionBody {
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    utm: Option<UtmAttributionBody>,
    #[serde(default)]
    meta: Option<MetaAttributionBody>,
}

/// Standard `utm_*` campaign tags, minus the redundant `utm_` prefix since
/// they are already namespaced under `utm`.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UtmAttributionBody {
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MetaAttributionBody {
    #[serde(default)]
    fbc: Option<String>,
    #[serde(default)]
    fbp: Option<String>,
}

/// `client_ip_address`/`client_user_agent` come from this request itself
/// (`X-Real-IP` is set by `deploy/nginx` from the real client address,
/// behind Cloudflare's realip module), never from the request body — the
/// browser has no trustworthy way to report either.
fn attribution_input(
    attribution: Option<&AttributionBody>,
    headers: &HeaderMap,
) -> Option<CheckoutAttributionInput> {
    let client_ip_address = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let client_user_agent = headers
        .get(axum::http::header::USER_AGENT)
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

mod create_cart {
    use super::*;

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct CreateCartBody {}

    pub(super) async fn handler(
        State(state): State<ApiState>,
        ShopperContext(actor): ShopperContext,
        ApiJson(CreateCartBody {}): ApiJson<CreateCartBody>,
    ) -> Result<ApiResponse<CartData>, ApiError> {
        let cart = state
            .storefront_sales
            .create_cart(CreateCartInput { actor })
            .await?;
        Ok(ApiResponse::created(cart_data(cart, None)?))
    }
}

// ===== GET /carts/{cart_id} =====

mod get_cart {
    use super::*;

    pub(super) async fn handler(
        State(state): State<ApiState>,
        ShopperContext(actor): ShopperContext,
        ApiPath(path): ApiPath<CartPath>,
    ) -> Result<ApiResponse<CartData>, ApiError> {
        let cart = state
            .storefront_sales
            .get_cart(&actor, CartId::from_uuid(path.cart_id))
            .await?;
        Ok(ApiResponse::ok(cart_data(cart, None)?))
    }
}

// ===== PUT /carts/{cart_id}/lines/{product_variant_id} =====

mod set_cart_line {
    use super::*;

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct SetCartLineBody {
        quantity: u32,
        /// Ad-platform attribution the browser read off its own cookies/URL,
        /// forwarded to the server-side Meta CAPI `AddToCart` event when this
        /// call raises the line quantity. Same shape as the checkout handler's.
        #[serde(default)]
        attribution: Option<AttributionBody>,
    }

    pub(super) async fn handler(
        State(state): State<ApiState>,
        headers: HeaderMap,
        ShopperContext(actor): ShopperContext,
        ApiPath(path): ApiPath<CartLinePath>,
        ApiJson(body): ApiJson<SetCartLineBody>,
    ) -> Result<ApiResponse<CartData>, ApiError> {
        let (cart, event_id) = state
            .storefront_sales
            .set_cart_line(SetCartLineInput {
                actor,
                cart_id: CartId::from_uuid(path.cart_id),
                product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
                quantity: body.quantity,
                now: state.clock.now(),
                attribution: attribution_input(body.attribution.as_ref(), &headers),
            })
            .await?;
        Ok(ApiResponse::ok(cart_data(cart, event_id)?))
    }
}

// ===== DELETE /carts/{cart_id}/lines/{product_variant_id} =====

mod remove_cart_line {
    use super::*;

    pub(super) async fn handler(
        State(state): State<ApiState>,
        ShopperContext(actor): ShopperContext,
        ApiPath(path): ApiPath<CartLinePath>,
    ) -> Result<ApiResponse<CartData>, ApiError> {
        let cart = state
            .storefront_sales
            .remove_cart_line(RemoveCartLineInput {
                actor,
                cart_id: CartId::from_uuid(path.cart_id),
                product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
            })
            .await?;
        Ok(ApiResponse::ok(cart_data(cart, None)?))
    }
}

// ===== POST /carts/{cart_id}/checkout =====

mod create_embedded_checkout {
    use super::*;

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct CreateEmbeddedCheckoutBody {
        return_url: String,
        payment_provider: String,
        #[serde(default)]
        attribution: Option<AttributionBody>,
    }

    #[derive(Serialize)]
    pub(super) struct EmbeddedCheckoutData {
        order_number: String,
        client_action: PaymentClientActionData,
        /// Shared with the browser Pixel's own InitiateCheckout call so Meta
        /// can deduplicate it against the server-side CAPI copy Chaos already
        /// sent.
        event_id: Uuid,
    }

    #[derive(Serialize)]
    pub(super) struct PaymentClientActionData {
        r#type: &'static str,
        public_key: String,
        client_token: String,
    }

    pub(super) async fn handler(
        State(state): State<ApiState>,
        headers: HeaderMap,
        ShopperContext(actor): ShopperContext,
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
                return_url: body.return_url.clone(),
                payment_provider,
                now: state.clock.now(),
                idempotency_key,
                attribution: attribution_input(body.attribution.as_ref(), &headers),
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
        Ok(ApiResponse::created(embedded_checkout_data(
            checkout,
            draft.event_id,
        )))
    }

    fn embedded_checkout_data(
        checkout: chaos_core::payments::EmbeddedCheckoutResult,
        event_id: Uuid,
    ) -> EmbeddedCheckoutData {
        EmbeddedCheckoutData {
            order_number: checkout.order_number,
            client_action: client_action_data(checkout.client_action),
            event_id,
        }
    }

    fn client_action_data(value: PaymentClientAction) -> PaymentClientActionData {
        PaymentClientActionData {
            r#type: value.kind,
            public_key: value.public_key.expose_secret().to_owned(),
            client_token: value.client_token.expose_secret().to_owned(),
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
}
