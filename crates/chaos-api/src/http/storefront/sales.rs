use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    ports::{
        CartDetail, CartLineItem, IdempotencyRequest, OrderDetail, OrderLineItem,
        StorefrontMediaAsset,
    },
    sales::{CreateCartInput, RemoveCartLineInput, SetCartLineInput},
};
use chaos_domain::{
    catalog::ProductVariantId,
    fulfillment::ShippingSelection,
    sales::{CartId, OrderId},
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::http::shared::pagination::idempotency_key;
use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, CartMachine, CartShopper,
    OrderLookupMachine,
};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/shopper-sessions", post(create_shopper_session))
        .route("/carts", post(create_cart))
        .route("/carts/{cart_id}", get(get_cart))
        .route(
            "/carts/{cart_id}/lines/{product_variant_id}",
            axum::routing::put(set_cart_line).delete(remove_cart_line),
        )
        .route("/orders/{order_id}", get(get_order))
        .route("/order-tracking-sessions", post(exchange_tracking_key))
        .route("/order-tracking-orders", post(get_tracked_order))
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateCartBody {
    currency: Option<String>,
    locale: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackingKeyBody {
    tracking_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackingSessionBody {
    access_token: String,
}

#[derive(Serialize)]
struct OrderTrackingSessionData {
    access_token: String,
    expires_at: ApiDateTime,
    order: OrderData,
}

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

#[derive(Deserialize)]
struct OrderPath {
    order_id: Uuid,
}

#[derive(Serialize)]
struct CartLineData {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    requires_shipping: bool,
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    media: Vec<CartMediaData>,
}

#[derive(Serialize)]
struct CartMediaData {
    id: Uuid,
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
    locale: String,
    status: &'static str,
    version: u64,
    lines: Vec<CartLineData>,
    subtotal_amount_minor: i64,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct ShopperSessionData {
    shopper_token: String,
}

#[derive(Serialize)]
struct OrderLineData {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    requires_shipping: bool,
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
}

#[derive(Serialize)]
struct OrderTransitionData {
    id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_status: Option<&'static str>,
    to_status: &'static str,
    kind: String,
    occurred_at: ApiDateTime,
}

#[derive(Serialize)]
pub(super) struct OrderData {
    id: Uuid,
    order_number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    locale: String,
    status: &'static str,
    fulfillment_status: &'static str,
    delivery_status: &'static str,
    contact: OrderContactData,
    #[serde(skip_serializing_if = "Option::is_none")]
    billing_address: Option<PostalAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_address: Option<PostalAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping: Option<ShippingSelectionData>,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    lines: Vec<OrderLineData>,
    transitions: Vec<OrderTransitionData>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct OrderContactData {
    email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<String>,
}

#[derive(Serialize)]
struct ShippingSelectionData {
    service_id: Uuid,
    code: String,
    name: String,
    amount_minor: i64,
    currency: String,
    estimated_min_days: u16,
    estimated_max_days: u16,
}

#[derive(Serialize)]
struct PostalAddressData {
    full_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    company: Option<String>,
    address_line1: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    address_line2: Option<String>,
    locality: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    administrative_area: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    postal_code: Option<String>,
    country_code: String,
}

async fn create_shopper_session(
    State(state): State<ApiState>,
    CartMachine(actor): CartMachine,
) -> Result<ApiResponse<ShopperSessionData>, ApiError> {
    let shopper_id = state.storefront_sales.create_shopper(&actor).await?;
    let shopper_token = state.shopper_credentials.issue(&actor, shopper_id)?;
    Ok(ApiResponse::created(ShopperSessionData {
        shopper_token: shopper_token.expose_secret().to_owned(),
    }))
}

async fn create_cart(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CartShopper(actor): CartShopper,
    ApiJson(body): ApiJson<CreateCartBody>,
) -> Result<ApiResponse<CartData>, ApiError> {
    let idempotency = body_request(&headers, "create_cart", &body)?;
    let cart = state
        .storefront_sales
        .create_cart(CreateCartInput {
            actor,
            currency: body.currency,
            locale: body.locale,
            idempotency,
        })
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
    let idempotency = body_request(
        &headers,
        "set_cart_line",
        &(path.cart_id, path.product_variant_id, &body),
    )?;
    let cart = state
        .storefront_sales
        .set_cart_line(SetCartLineInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
            quantity: body.quantity,
            idempotency,
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
    let idempotency = body_request(
        &headers,
        "remove_cart_line",
        &(path.cart_id, path.product_variant_id),
    )?;
    let cart = state
        .storefront_sales
        .remove_cart_line(RemoveCartLineInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            product_variant_id: ProductVariantId::from_uuid(path.product_variant_id),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(cart_data(cart)?))
}

async fn get_order(
    State(state): State<ApiState>,
    CartShopper(actor): CartShopper,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    let order = state
        .storefront_sales
        .get_order(&actor, OrderId::from_uuid(path.order_id))
        .await?;
    Ok(ApiResponse::ok(order_data(order)?))
}

async fn exchange_tracking_key(
    State(state): State<ApiState>,
    OrderLookupMachine(actor): OrderLookupMachine,
    ApiJson(body): ApiJson<TrackingKeyBody>,
) -> Result<ApiResponse<OrderTrackingSessionData>, ApiError> {
    let session = state
        .storefront_sales
        .exchange_order_tracking_key(
            &actor,
            &secrecy::SecretString::from(body.tracking_key),
            state.clock.now(),
        )
        .await?;
    Ok(ApiResponse::created(OrderTrackingSessionData {
        access_token: session.access_token.expose_secret().to_owned(),
        expires_at: session.expires_at.into(),
        order: order_data(session.order)?,
    }))
}

async fn get_tracked_order(
    State(state): State<ApiState>,
    OrderLookupMachine(actor): OrderLookupMachine,
    ApiJson(body): ApiJson<TrackingSessionBody>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    let order = state
        .storefront_sales
        .get_tracked_order(
            &actor,
            &secrecy::SecretString::from(body.access_token),
            state.clock.now(),
        )
        .await?;
    Ok(ApiResponse::ok(order_data(order)?))
}

fn contact_data(value: &chaos_domain::sales::OrderContact) -> OrderContactData {
    OrderContactData {
        email: value.email().into(),
        phone: value.phone().map(str::to_owned),
    }
}

fn address_data(value: &chaos_domain::sales::PostalAddress) -> PostalAddressData {
    PostalAddressData {
        full_name: value.full_name().into(),
        company: value.company().map(str::to_owned),
        address_line1: value.address_line1().into(),
        address_line2: value.address_line2().map(str::to_owned),
        locality: value.locality().into(),
        administrative_area: value.administrative_area().map(str::to_owned),
        postal_code: value.postal_code().map(str::to_owned),
        country_code: value.country_code().into(),
    }
}

fn shipping_data(value: &ShippingSelection) -> ShippingSelectionData {
    ShippingSelectionData {
        service_id: value.service_id().as_uuid(),
        code: value.code().into(),
        name: value.name().into(),
        amount_minor: value.amount().amount_minor(),
        currency: value.amount().currency().as_str().into(),
        estimated_min_days: value.estimated_min_days(),
        estimated_max_days: value.estimated_max_days(),
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

fn cart_data(cart: CartDetail) -> Result<CartData, ApplicationError> {
    Ok(CartData {
        id: cart.id.as_uuid(),
        price_list_id: cart.price_list_id.as_uuid(),
        currency: cart.currency.as_str().to_owned(),
        locale: cart.locale.as_str().to_owned(),
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
        requires_shipping: line.requires_shipping,
        track_inventory: line.track_inventory,
        quantity: line.quantity,
        unit_price_amount_minor: line.unit_price_amount_minor,
        subtotal_amount_minor: line.subtotal_amount_minor,
        media: line.media.into_iter().map(cart_media_data).collect(),
    }
}

fn cart_media_data(media: StorefrontMediaAsset) -> CartMediaData {
    CartMediaData {
        id: media.id.as_uuid(),
        product_variant_id: media.product_variant_id.map(|id| id.as_uuid()),
        media_type: media.media_type,
        kind: media.kind.as_str(),
        alt_text: media.alt_text,
        position: media.position,
        url: media.url,
    }
}

pub(super) fn order_data(order: OrderDetail) -> Result<OrderData, ApplicationError> {
    Ok(OrderData {
        id: order.id.as_uuid(),
        order_number: order.order_number.as_str().into(),
        inventory_reservation_id: order.inventory_reservation_id.map(|id| id.as_uuid()),
        price_list_id: order.price_list_id.as_uuid(),
        currency: order.currency.as_str().to_owned(),
        locale: order.locale.as_str().to_owned(),
        status: order.status.as_str(),
        fulfillment_status: order.fulfillment_status.as_str(),
        delivery_status: order.delivery_status.as_str(),
        contact: contact_data(order.identity.contact()),
        billing_address: order.identity.billing_address().map(address_data),
        shipping_address: order.identity.shipping_address().map(address_data),
        shipping: order.shipping.as_ref().map(shipping_data),
        subtotal_amount_minor: order.subtotal_amount_minor,
        discount_amount_minor: order.discount_amount_minor,
        tax_amount_minor: order.tax_amount_minor,
        shipping_amount_minor: order.shipping_amount_minor,
        total_amount_minor: order.total_amount_minor,
        lines: order.lines.into_iter().map(order_line_data).collect(),
        transitions: order
            .transitions
            .into_iter()
            .map(|transition| {
                Ok(OrderTransitionData {
                    id: transition.id,
                    from_status: transition.from_status.map(|status| status.as_str()),
                    to_status: transition.to_status.as_str(),
                    kind: transition.kind,
                    occurred_at: transition.occurred_at.into(),
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        created_at: order.created_at.into(),
        updated_at: order.updated_at.into(),
    })
}

fn order_line_data(line: OrderLineItem) -> OrderLineData {
    OrderLineData {
        product_id: line.product_id.as_uuid(),
        product_variant_id: line.product_variant_id.as_uuid(),
        product_title: line.product_title,
        variant_title: line.variant_title,
        sku: line.sku,
        requires_shipping: line.requires_shipping,
        track_inventory: line.track_inventory,
        quantity: line.quantity,
        unit_price_amount_minor: line.unit_price_amount_minor,
        subtotal_amount_minor: line.subtotal_amount_minor,
    }
}
