use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    ports::{
        CartDetail, CartLineItem, CheckoutDetail, CheckoutLineItem, IdempotencyRequest,
        OrderDetail, OrderLineItem,
    },
    sales::{
        CheckoutContactInput, CreateCartInput, CreateCheckoutInput, CreateOrderInput,
        PostalAddressInput, QuoteShippingInput, RemoveCartLineInput, SetCartLineInput,
    },
};
use chaos_domain::{
    catalog::ProductVariantId,
    fulfillment::{ShippingSelection, ShippingServiceId},
    pricing::{PromotionSnapshot, TaxRuleSnapshot},
    sales::{CartId, CheckoutId, OrderId, ShopperId},
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, CartMachine, CartShopper,
    CheckoutShopper, OrderLookupMachine, pagination::idempotency_key,
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/shopper-sessions", post(create_shopper_session))
        .route("/carts", post(create_cart))
        .route("/carts/{cart_id}", get(get_cart))
        .route(
            "/carts/{cart_id}/lines/{product_variant_id}",
            axum::routing::put(set_cart_line).delete(remove_cart_line),
        )
        .route("/carts/{cart_id}/checkout", post(create_checkout))
        .route("/carts/{cart_id}/shipping-options", post(quote_shipping))
        .route("/checkouts/{checkout_id}", get(get_checkout))
        .route("/checkouts/{checkout_id}/order", post(create_order))
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckoutContactBody {
    email: String,
    phone: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PostalAddressBody {
    full_name: String,
    company: Option<String>,
    address_line1: String,
    address_line2: Option<String>,
    locality: String,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateCheckoutBody {
    contact: CheckoutContactBody,
    billing_address: PostalAddressBody,
    shipping_address: Option<PostalAddressBody>,
    shipping_service_id: Option<Uuid>,
    promotion_code: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QuoteShippingBody {
    destination_country: String,
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
struct CheckoutPath {
    checkout_id: Uuid,
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
    tax_inclusive: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    shopper_token: Option<String>,
}

#[derive(Serialize)]
struct ShopperSessionData {
    shopper_token: String,
}

#[derive(Serialize)]
struct CheckoutLineData {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    requires_shipping: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    tax_inclusive: bool,
}

#[derive(Serialize)]
struct CheckoutData {
    id: Uuid,
    cart_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    customer_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    locale: String,
    status: String,
    contact: CheckoutContactData,
    billing_address: PostalAddressData,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_address: Option<PostalAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping: Option<ShippingSelectionData>,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    tax_rule: TaxCalculationData,
    #[serde(skip_serializing_if = "Option::is_none")]
    promotion: Option<PromotionCalculationData>,
    tax_inclusive: bool,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    expires_at: ApiDateTime,
    lines: Vec<CheckoutLineData>,
    created_at: ApiDateTime,
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
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    tax_inclusive: bool,
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
    checkout_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    customer_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    locale: String,
    status: &'static str,
    fulfillment_status: &'static str,
    delivery_status: &'static str,
    contact: CheckoutContactData,
    billing_address: PostalAddressData,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_address: Option<PostalAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping: Option<ShippingSelectionData>,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    tax_rule: TaxCalculationData,
    #[serde(skip_serializing_if = "Option::is_none")]
    promotion: Option<PromotionCalculationData>,
    tax_inclusive: bool,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    lines: Vec<OrderLineData>,
    transitions: Vec<OrderTransitionData>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct CheckoutContactData {
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
struct TaxCalculationData {
    rule_id: Uuid,
    code: String,
    name: String,
    country_code: String,
    rate_basis_points: u32,
}

#[derive(Serialize)]
struct PromotionCalculationData {
    promotion_id: Uuid,
    handle: String,
    name: String,
    trigger: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    redemption_code: Option<String>,
    value_kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_basis_points: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum_amount_minor: Option<i64>,
    currency: String,
    minimum_subtotal_amount_minor: i64,
    priority: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    starts_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ends_at: Option<ApiDateTime>,
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
    let shopper_token = state.shopper_credentials.issue(&actor, ShopperId::new())?;
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
    let machine = actor.machine.clone();
    let cart = state
        .storefront_sales
        .create_cart(CreateCartInput {
            actor,
            currency: body.currency,
            locale: body.locale,
            idempotency,
        })
        .await?;
    let shopper_token = state.shopper_credentials.issue(&machine, cart.shopper_id)?;
    Ok(ApiResponse::created(cart_data(
        cart,
        Some(shopper_token.expose_secret().to_owned()),
    )?))
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
    Ok(ApiResponse::ok(cart_data(cart, None)?))
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
    Ok(ApiResponse::ok(cart_data(cart, None)?))
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
    Ok(ApiResponse::ok(cart_data(cart, None)?))
}

async fn create_checkout(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<CartPath>,
    ApiJson(body): ApiJson<CreateCheckoutBody>,
) -> Result<ApiResponse<CheckoutData>, ApiError> {
    let idempotency = body_request(&headers, "create_checkout", &(path.cart_id, &body))?;
    let checkout = state
        .storefront_sales
        .create_checkout(CreateCheckoutInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            contact: contact_input(body.contact),
            billing_address: address_input(body.billing_address),
            shipping_address: body.shipping_address.map(address_input),
            shipping_service_id: body.shipping_service_id.map(ShippingServiceId::from_uuid),
            promotion_code: body.promotion_code,
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(checkout_data(checkout)?))
}

async fn quote_shipping(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<CartPath>,
    ApiJson(body): ApiJson<QuoteShippingBody>,
) -> Result<ApiResponse<Vec<ShippingSelectionData>>, ApiError> {
    let quotes = state
        .storefront_sales
        .quote_shipping(QuoteShippingInput {
            actor,
            cart_id: CartId::from_uuid(path.cart_id),
            destination_country: body.destination_country,
        })
        .await?;
    Ok(ApiResponse::ok(quotes.iter().map(shipping_data).collect()))
}

async fn get_checkout(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<CheckoutPath>,
) -> Result<ApiResponse<CheckoutData>, ApiError> {
    let checkout = state
        .storefront_sales
        .get_checkout(&actor, CheckoutId::from_uuid(path.checkout_id))
        .await?;
    Ok(ApiResponse::ok(checkout_data(checkout)?))
}

async fn create_order(
    State(state): State<ApiState>,
    headers: HeaderMap,
    CheckoutShopper(actor): CheckoutShopper,
    ApiPath(path): ApiPath<CheckoutPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    let idempotency = body_request(&headers, "create_order", &path.checkout_id)?;
    let order = state
        .storefront_sales
        .create_order(CreateOrderInput {
            actor,
            checkout_id: CheckoutId::from_uuid(path.checkout_id),
            now: state.clock.now(),
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(order_data(order)?))
}

async fn get_order(
    State(state): State<ApiState>,
    CheckoutShopper(actor): CheckoutShopper,
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

fn contact_input(value: CheckoutContactBody) -> CheckoutContactInput {
    CheckoutContactInput {
        email: value.email,
        phone: value.phone,
    }
}

fn address_input(value: PostalAddressBody) -> PostalAddressInput {
    PostalAddressInput {
        full_name: value.full_name,
        company: value.company,
        address_line1: value.address_line1,
        address_line2: value.address_line2,
        locality: value.locality,
        administrative_area: value.administrative_area,
        postal_code: value.postal_code,
        country_code: value.country_code,
    }
}

fn contact_data(value: &chaos_domain::sales::CheckoutContact) -> CheckoutContactData {
    CheckoutContactData {
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

fn tax_data(value: &TaxRuleSnapshot) -> TaxCalculationData {
    TaxCalculationData {
        rule_id: value.rule_id().as_uuid(),
        code: value.code().into(),
        name: value.name().into(),
        country_code: value.country_code().into(),
        rate_basis_points: value.rate_basis_points(),
    }
}

fn promotion_data(value: &PromotionSnapshot) -> PromotionCalculationData {
    PromotionCalculationData {
        promotion_id: value.promotion_id().as_uuid(),
        handle: value.handle().into(),
        name: value.name().into(),
        trigger: value.trigger().as_str(),
        redemption_code: value.redemption_code().map(Into::into),
        value_kind: value.value().kind(),
        rate_basis_points: value.value().rate_basis_points(),
        amount_minor: value.value().amount_minor(),
        maximum_amount_minor: value.value().maximum_amount_minor(),
        currency: value.currency().as_str().into(),
        minimum_subtotal_amount_minor: value.minimum_subtotal_amount_minor(),
        priority: value.priority(),
        starts_at: value.starts_at().map(Into::into),
        ends_at: value.ends_at().map(Into::into),
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

fn cart_data(
    cart: CartDetail,
    shopper_token: Option<String>,
) -> Result<CartData, ApplicationError> {
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
        shopper_token,
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
        tax_inclusive: line.tax_inclusive,
    }
}

fn checkout_data(checkout: CheckoutDetail) -> Result<CheckoutData, ApplicationError> {
    Ok(CheckoutData {
        id: checkout.id.as_uuid(),
        cart_id: checkout.cart_id.as_uuid(),
        customer_id: checkout.customer_id.map(|id| id.as_uuid()),
        inventory_reservation_id: checkout.inventory_reservation_id.map(|id| id.as_uuid()),
        price_list_id: checkout.price_list_id.as_uuid(),
        currency: checkout.currency.as_str().to_owned(),
        locale: checkout.locale.as_str().to_owned(),
        status: checkout.status,
        contact: contact_data(checkout.identity.contact()),
        billing_address: address_data(checkout.identity.billing_address()),
        shipping_address: checkout.identity.shipping_address().map(address_data),
        shipping: checkout.shipping.as_ref().map(shipping_data),
        subtotal_amount_minor: checkout.subtotal_amount_minor,
        discount_amount_minor: checkout.discount_amount_minor,
        tax_amount_minor: checkout.tax_amount_minor,
        tax_rule: tax_data(&checkout.tax_rule),
        promotion: checkout.promotion.as_ref().map(promotion_data),
        tax_inclusive: checkout.tax_inclusive,
        shipping_amount_minor: checkout.shipping_amount_minor,
        total_amount_minor: checkout.total_amount_minor,
        expires_at: checkout.expires_at.into(),
        lines: checkout.lines.into_iter().map(checkout_line_data).collect(),
        created_at: checkout.created_at.into(),
    })
}

fn checkout_line_data(line: CheckoutLineItem) -> CheckoutLineData {
    CheckoutLineData {
        product_id: line.product_id.as_uuid(),
        product_variant_id: line.product_variant_id.as_uuid(),
        product_title: line.product_title,
        variant_title: line.variant_title,
        sku: line.sku,
        requires_shipping: line.requires_shipping,
        quantity: line.quantity,
        unit_price_amount_minor: line.unit_price_amount_minor,
        subtotal_amount_minor: line.subtotal_amount_minor,
        discount_amount_minor: line.discount_amount_minor,
        tax_amount_minor: line.tax_amount_minor,
        total_amount_minor: line.total_amount_minor,
        tax_inclusive: line.tax_inclusive,
    }
}

pub(super) fn order_data(order: OrderDetail) -> Result<OrderData, ApplicationError> {
    Ok(OrderData {
        id: order.id.as_uuid(),
        order_number: order.order_number.as_str().into(),
        checkout_id: order.checkout_id.as_uuid(),
        customer_id: order.customer_id.map(|id| id.as_uuid()),
        inventory_reservation_id: order.inventory_reservation_id.map(|id| id.as_uuid()),
        price_list_id: order.price_list_id.as_uuid(),
        currency: order.currency.as_str().to_owned(),
        locale: order.locale.as_str().to_owned(),
        status: order.status.as_str(),
        fulfillment_status: order.fulfillment_status.as_str(),
        delivery_status: order.delivery_status.as_str(),
        contact: contact_data(order.identity.contact()),
        billing_address: address_data(order.identity.billing_address()),
        shipping_address: order.identity.shipping_address().map(address_data),
        shipping: order.shipping.as_ref().map(shipping_data),
        subtotal_amount_minor: order.subtotal_amount_minor,
        discount_amount_minor: order.discount_amount_minor,
        tax_amount_minor: order.tax_amount_minor,
        tax_rule: tax_data(&order.tax_rule),
        promotion: order.promotion.as_ref().map(promotion_data),
        tax_inclusive: order.tax_inclusive,
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
        discount_amount_minor: line.discount_amount_minor,
        tax_amount_minor: line.tax_amount_minor,
        total_amount_minor: line.total_amount_minor,
        tax_inclusive: line.tax_inclusive,
    }
}
