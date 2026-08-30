use axum::{
    Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post, put},
};
use chaos_core::{
    ApplicationError,
    contracts::{CartDetail, CartLineItem, StorefrontMediaAsset, StorefrontMediaScope},
    sales::{CreateCartInput, RemoveCartLineInput, SetCartLineInput},
};
use chaos_domain::{catalog::ProductVariantId, sales::CartId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, ShopperContext, invalid_value,
};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/carts", post(create_cart))
        .route("/carts/{cart_id}", get(get_cart))
        .route("/carts/{cart_id}/lines/{product_variant_id}", put(set_cart_line).delete(remove_cart_line))
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
    ShopperContext(actor): ShopperContext,
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
    ShopperContext(actor): ShopperContext,
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
    ShopperContext(actor): ShopperContext,
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
    ShopperContext(actor): ShopperContext,
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
