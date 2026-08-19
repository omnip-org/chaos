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
    CheckoutShopper, OrderLookupMachine, merchant::idempotency_key,
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
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateCartBody {
    currency: Option<String>,
    locale: Option<String>,
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
    OrderLookupMachine(actor): OrderLookupMachine,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<ApiResponse<OrderData>, ApiError> {
    let order = state
        .storefront_sales
        .get_order_by_id(&actor, OrderId::from_uuid(path.order_id))
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{HeaderValue, Method, Request, StatusCode},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use chaos_application::analytics::{AnalyticsCollection, AnalyticsWorkers};
    use chaos_application::ports::{
        AnalyticsAttributionQueue, AnalyticsCollectionRateLimiter, AnalyticsDestination,
        AnalyticsDestinationError, AnalyticsExportCommand, AnalyticsExportReceipt,
        AnalyticsRateLimitDecision, AnalyticsSessionizationQueue, ApiKeyMaterialGenerator,
        GeneratedApiKeyMaterial,
    };
    use chaos_application::{
        ApplicationError,
        fulfillment::{FulfillmentWorkers, ShippingProviderAdministration},
        payments::{PaymentProviderAdministration, PaymentWorkers},
        ports::{
            CancelShippingLabelCommand, IntegrationQueue, PaymentProviderOnboarding,
            PaymentProviderReadiness, PaymentProviderReadinessQueue, PaymentRepository,
            ProviderCommandResult, ProviderTrackingStatus, PurchaseShippingLabelCommand,
            PurchasedShippingLabel, QueueJob, ReconcileShippingLabelCommand,
            ReconciledShippingLabel, RefreshTrackingCommand, ShippingCancellationStatus,
            ShippingProvider, ShippingQuoteCommand, ShippingRateQuote, ShippingTrackingQueue,
            ShippingTrackingSnapshot,
        },
    };
    use chaos_domain::{
        CurrencyCode,
        analytics::{AnalyticsDestinationProvider, SessionEventContribution},
        catalog::{ProductId, ProductVariantId},
        fulfillment::ShippingServiceId,
        identity::UserId,
        inventory::InventoryLocationId,
        merchant::{ApiKeyClass, ApiKeyId, SalesChannelId, StoreId},
        payments::PaymentSecretReference,
        pricing::PriceListId,
    };
    use chaos_infrastructure::repositories::SecureApiKeyMaterialGenerator;
    use chaos_infrastructure::repositories::{
        PostgresAnalyticsEventRepository, PostgresFulfillmentRepository, PostgresPaymentRepository,
        PostgresShippingServiceRepository, SandboxPaymentProvider,
    };
    use hmac::{Hmac, KeyInit, Mac};
    use secrecy::ExposeSecret;
    use serde_json::{Value, json};
    use sha2::Sha256;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    struct ActionRequiredPaymentOnboarding;

    struct AllowAnalyticsCollection;

    struct RecordingAnalyticsDestination {
        provider: AnalyticsDestinationProvider,
        deliveries: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AnalyticsDestination for RecordingAnalyticsDestination {
        fn provider(&self) -> AnalyticsDestinationProvider {
            self.provider
        }

        async fn send(
            &self,
            command: &AnalyticsExportCommand,
        ) -> Result<AnalyticsExportReceipt, AnalyticsDestinationError> {
            assert_eq!(command.provider, self.provider);
            assert!(!command.order_id.is_nil());
            assert!(!command.anonymous_id.is_nil());
            assert!(!command.items.is_empty());
            self.deliveries.fetch_add(1, Ordering::SeqCst);
            Ok(AnalyticsExportReceipt {
                provider_reference: command.fact_id.to_string(),
            })
        }
    }

    #[async_trait]
    impl AnalyticsCollectionRateLimiter for AllowAnalyticsCollection {
        async fn consume(
            &self,
            _store_id: StoreId,
            _sales_channel_id: SalesChannelId,
            _anonymous_event_counts: &[(Uuid, u16)],
            _event_count: u16,
        ) -> Result<AnalyticsRateLimitDecision, ApplicationError> {
            Ok(AnalyticsRateLimitDecision {
                allowed: true,
                retry_after_seconds: 60,
            })
        }
    }

    #[async_trait]
    impl PaymentProviderOnboarding for ActionRequiredPaymentOnboarding {
        fn name(&self) -> &'static str {
            "blockedpay"
        }

        async fn check_readiness(
            &self,
            external_account_reference: &str,
            _credential_secret_reference: &PaymentSecretReference,
            checked_at: time::OffsetDateTime,
        ) -> Result<PaymentProviderReadiness, ApplicationError> {
            Ok(PaymentProviderReadiness {
                ready: false,
                blocker_codes: vec!["account_incomplete".into()],
                configuration: json!({
                    "account_reference": external_account_reference,
                    "ready": false,
                    "blocker_codes": ["account_incomplete"]
                }),
                checked_at,
            })
        }
    }

    struct UnavailableTestpayOnboarding;

    #[async_trait]
    impl PaymentProviderOnboarding for UnavailableTestpayOnboarding {
        fn name(&self) -> &'static str {
            "testpay"
        }

        async fn check_readiness(
            &self,
            _external_account_reference: &str,
            _credential_secret_reference: &PaymentSecretReference,
            _checked_at: time::OffsetDateTime,
        ) -> Result<PaymentProviderReadiness, ApplicationError> {
            Err(ApplicationError::Unavailable {
                service: "testpay",
                source: anyhow::anyhow!("simulated readiness dependency failure"),
            })
        }
    }

    struct TestShippingProvider {
        purchases: Arc<AtomicUsize>,
        cancellation_requested: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ShippingProvider for TestShippingProvider {
        fn name(&self) -> &'static str {
            "easypost"
        }

        async fn quote_rates(
            &self,
            _command: ShippingQuoteCommand,
        ) -> Result<Vec<ShippingRateQuote>, ApplicationError> {
            Ok(vec![ShippingRateQuote {
                provider_shipment_reference: "shp_test123".into(),
                provider_rate_reference: "rate_test123".into(),
                carrier: "USPS".into(),
                service: "GroundAdvantage".into(),
                amount_minor: 1_234,
                currency: CurrencyCode::USD,
                estimated_delivery_days: Some(4),
                guaranteed: false,
            }])
        }

        async fn purchase_label(
            &self,
            _command: PurchaseShippingLabelCommand,
        ) -> Result<PurchasedShippingLabel, ApplicationError> {
            self.purchases.fetch_add(1, Ordering::SeqCst);
            Ok(PurchasedShippingLabel {
                provider_shipment_reference: "shp_test123".into(),
                carrier: "USPS".into(),
                tracking_number: "9400111899223856928499".into(),
                provider_tracker_reference: Some("trk_test123".into()),
                label_url: "https://labels.example.com/test.png".into(),
                label_media_type: "image/png".into(),
            })
        }

        async fn reconcile_label(
            &self,
            _command: ReconcileShippingLabelCommand,
        ) -> Result<ReconciledShippingLabel, ApplicationError> {
            Ok(ReconciledShippingLabel {
                label: Some(PurchasedShippingLabel {
                    provider_shipment_reference: "shp_test123".into(),
                    carrier: "USPS".into(),
                    tracking_number: "9400111899223856928499".into(),
                    provider_tracker_reference: Some("trk_test123".into()),
                    label_url: "https://labels.example.com/test.png".into(),
                    label_media_type: "image/png".into(),
                }),
                cancellation_status: self
                    .cancellation_requested
                    .load(Ordering::SeqCst)
                    .then_some(ShippingCancellationStatus::Cancelled),
            })
        }

        async fn cancel_label(
            &self,
            _command: CancelShippingLabelCommand,
        ) -> Result<ShippingCancellationStatus, ApplicationError> {
            self.cancellation_requested.store(true, Ordering::SeqCst);
            Ok(ShippingCancellationStatus::Submitted)
        }

        async fn refresh_tracking(
            &self,
            _command: RefreshTrackingCommand,
            observed_at: time::OffsetDateTime,
        ) -> Result<ShippingTrackingSnapshot, ApplicationError> {
            Ok(ShippingTrackingSnapshot {
                status: ProviderTrackingStatus::Delivered,
                status_detail: Some("delivered".into()),
                estimated_delivery_at: Some(observed_at),
                observed_at,
            })
        }
    }

    async fn insert_key(
        pool: &PgPool,
        store_id: StoreId,
        user_id: UserId,
        scopes: &[&str],
    ) -> GeneratedApiKeyMaterial {
        let material = SecureApiKeyMaterialGenerator.generate(ApiKeyClass::Publishable);
        let key_id = ApiKeyId::new();
        sqlx::query(
            "INSERT INTO merchant.api_keys \
             (id, store_id, key_identifier, secret_digest, \
              display_suffix, name, class, created_by_user_id) \
             VALUES ($1, $2, $3, $4, $5, 'Sales HTTP', 'publishable', $6)",
        )
        .bind(key_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(&material.key_identifier)
        .bind(material.secret_digest.as_slice())
        .bind(&material.display_suffix)
        .bind(user_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
        for scope in scopes {
            sqlx::query(
                "INSERT INTO merchant.api_key_scopes (api_key_id, scope) \
                 VALUES ($1, $2::merchant.api_key_scope)",
            )
            .bind(key_id.as_uuid())
            .bind(scope)
            .execute(pool)
            .await
            .unwrap();
        }
        material
    }

    fn store_request(
        method: Method,
        uri: &str,
        secret: Option<&str>,
        idempotency_key: Option<&str>,
        body: Option<Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(secret) = secret {
            builder = builder.header("authorization", format!("Bearer {secret}"));
        }
        if let Some(key) = idempotency_key {
            builder = builder.header("idempotency-key", key);
        }
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        builder
            .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
            .unwrap()
    }

    fn webhook_request(event: Value) -> Request<Body> {
        let body = event.to_string();
        let mut mac =
            Hmac::<Sha256>::new_from_slice(b"test-payment-webhook-secret-32-bytes").unwrap();
        mac.update(body.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        Request::post("/webhooks/v1/payments/testpay")
            .header("content-type", "application/json")
            .header("x-payment-signature", signature)
            .body(Body::from(body))
            .unwrap()
    }

    fn with_shopper_token(mut request: Request<Body>, shopper_token: &str) -> Request<Body> {
        request.headers_mut().insert(
            "x-chaos-shopper-token",
            HeaderValue::from_str(shopper_token).unwrap(),
        );
        request
    }

    fn with_customer_session(mut request: Request<Body>) -> Request<Body> {
        request.headers_mut().insert(
            "x-chaos-customer-session",
            HeaderValue::from_static("customer-session"),
        );
        request
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn storefront_cart_and_checkout_http_matrix_is_scoped_idempotent_and_inventory_safe() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let suffix = Uuid::now_v7().simple().to_string();
        let provider_account_reference = format!("acct_{suffix}");
        let user_id = UserId::new();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let other_channel_id = SalesChannelId::new();
        let product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let price_list_id = PriceListId::new();
        let location_id = InventoryLocationId::new();
        let shipping_service_id = ShippingServiceId::new();
        let tax_rule_id = Uuid::now_v7();
        let testpay_provider_account_id = Uuid::now_v7();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id.as_uuid())
            .bind(format!("sales-http-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO merchant.stores (id, code, name, status) \
             VALUES ($1, $2, 'Other Sales HTTP', 'active')",
        )
        .bind(other_store_id.as_uuid())
        .bind(format!("sales-o-{}", &suffix[12..28]))
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.stores (id, code, name, status) \
             VALUES ($1, $2, 'Sales HTTP', 'active')",
        )
        .bind(store_id.as_uuid())
        .bind(format!("sales-{}", &suffix[12..28]))
        .execute(&owner_pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO merchant.store_currencies (store_id, currency) \
             VALUES ($1, 'USD')",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.store_memberships \
             (store_id, user_id, role) VALUES ($1, $2, 'owner')",
        )
        .bind(store_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, 'web', 'Web', 'web', true)",
        )
        .bind(other_channel_id.as_uuid())
        .bind(other_store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.promotions \
             (id, store_id, handle, name, trigger, redemption_code, \
              value_kind, amount_minor, currency, minimum_subtotal_amount_minor, priority) \
             VALUES ($1, $2, 'save-three', 'Save three dollars', 'code', 'SAVE300', \
                     'fixed_amount', 300, 'USD', 1000, 10)",
        )
        .bind(Uuid::now_v7())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.promotions \
             (id, store_id, handle, name, trigger, value_kind, \
              rate_basis_points, currency, priority) \
             VALUES ($1, $2, 'automatic-ten', 'Automatic ten percent', 'automatic', \
                     'percentage', 1000, 'USD', 5)",
        )
        .bind(Uuid::now_v7())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO fulfillment.shipping_services \
             (id, store_id, code, name, amount_minor, currency, \
              estimated_min_days, estimated_max_days) \
             VALUES ($1, $2, 'standard', 'Standard shipping', 500, 'USD', 2, 5)",
        )
        .bind(shipping_service_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO fulfillment.shipping_service_regions \
             (store_id, shipping_service_id, country_code) \
             VALUES ($1, $2, 'US')",
        )
        .bind(store_id.as_uuid())
        .bind(shipping_service_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.tax_rules \
             (id, store_id, code, name, country_code, rate_basis_points) \
             VALUES ($1, $2, 'us-sales-tax', 'US sales tax', 'US', 900)",
        )
        .bind(tax_rule_id)
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, 'web', 'Web', 'web', true)",
        )
        .bind(channel_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO payments.provider_accounts \
             (id, store_id, provider, external_account_reference, \
              credential_secret_reference, webhook_secret_reference, readiness_status, \
              readiness_snapshot, readiness_checked_at, readiness_valid_until, \
              readiness_reconcile_at, enabled) \
             VALUES ($1, $2, 'testpay', $3, 'test://credential', 'test://webhook', \
                     'ready', '{\"blocker_codes\":[]}'::jsonb, CURRENT_TIMESTAMP, \
                     CURRENT_TIMESTAMP + INTERVAL '24 hours', \
                     CURRENT_TIMESTAMP + INTERVAL '6 hours', true)",
        )
        .bind(testpay_provider_account_id)
        .bind(store_id.as_uuid())
        .bind(&provider_account_reference)
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.products \
             (id, store_id, handle, title, status) \
             VALUES ($1, $2, 'sales-product', 'Sales Product', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, store_id, product_id, title, status, track_inventory) \
             VALUES ($1, $2, $3, 'Default', 'active', true)",
        )
        .bind(variant_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_publications \
             (store_id, product_id, sales_channel_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(channel_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.price_lists \
             (id, store_id, code, name, currency, tax_inclusive, status) \
             VALUES ($1, $2, 'usd', 'USD', 'USD', true, 'active')",
        )
        .bind(price_list_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.prices \
             (id, store_id, price_list_id, product_variant_id, amount_minor) \
             VALUES ($1, $2, $3, $4, 1250)",
        )
        .bind(Uuid::now_v7())
        .bind(store_id.as_uuid())
        .bind(price_list_id.as_uuid())
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO inventory.inventory_locations \
             (id, store_id, code, name) \
             VALUES ($1, $2, 'primary', 'Primary')",
        )
        .bind(location_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO inventory.stock_items \
             (id, store_id, inventory_location_id, product_variant_id, \
              on_hand_quantity) VALUES ($1, $2, $3, $4, 2)",
        )
        .bind(Uuid::now_v7())
        .bind(store_id.as_uuid())
        .bind(location_id.as_uuid())
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let full_key = insert_key(
            &owner_pool,
            store_id,
            user_id,
            &["analytics:write", "carts:write", "checkout:write"],
        )
        .await;
        let catalog_key = insert_key(&owner_pool, store_id, user_id, &["catalog:read"]).await;
        let other_store_key = insert_key(
            &owner_pool,
            other_store_id,
            user_id,
            &["analytics:write", "carts:write", "checkout:write"],
        )
        .await;
        let full_secret = full_key.plaintext.expose_secret();
        let catalog_secret = catalog_key.plaintext.expose_secret();
        let other_store_secret = other_store_key.plaintext.expose_secret();
        let mut state = test_state(&database_url, user_id);
        let test_clock = state.clock.clone();
        let runtime_pool = state.infrastructure.runtime_pool();
        let reporting_pool = state.infrastructure.analytics_pool();
        let analytics_repository =
            Arc::new(PostgresAnalyticsEventRepository::new(runtime_pool.clone()));
        let analytics_delivery_count = Arc::new(AtomicUsize::new(0));
        state.analytics_workers = Arc::new(AnalyticsWorkers::new(
            analytics_repository.clone(),
            analytics_repository.clone(),
            analytics_repository.clone(),
            analytics_repository.clone(),
            analytics_repository.clone(),
            vec![
                Arc::new(RecordingAnalyticsDestination {
                    provider: AnalyticsDestinationProvider::MetaCapi,
                    deliveries: analytics_delivery_count.clone(),
                }),
                Arc::new(RecordingAnalyticsDestination {
                    provider: AnalyticsDestinationProvider::Ga4,
                    deliveries: analytics_delivery_count.clone(),
                }),
            ],
        ));
        let analytics_workers = state.analytics_workers.clone();
        state.analytics_collection = Arc::new(AnalyticsCollection::new(
            Arc::new(PostgresAnalyticsEventRepository::new(runtime_pool.clone())),
            Arc::new(AllowAnalyticsCollection),
        ));
        let payment_repository = Arc::new(PostgresPaymentRepository::new(runtime_pool.clone()));
        let shipping_repository =
            Arc::new(PostgresShippingServiceRepository::new(runtime_pool.clone()));
        let shipping_purchase_count = Arc::new(AtomicUsize::new(0));
        let shipping_cancellation_requested = Arc::new(AtomicBool::new(false));
        let shipping_provider: Arc<dyn ShippingProvider> = Arc::new(TestShippingProvider {
            purchases: shipping_purchase_count.clone(),
            cancellation_requested: shipping_cancellation_requested,
        });
        state.shipping_provider_administration = Arc::new(ShippingProviderAdministration::new(
            shipping_repository.clone(),
            shipping_repository.clone(),
            [shipping_provider.clone()],
        ));
        state.payment_provider_administration = Arc::new(PaymentProviderAdministration::new(
            payment_repository.clone(),
            vec![
                Arc::new(SandboxPaymentProvider) as Arc<dyn PaymentProviderOnboarding>,
                Arc::new(ActionRequiredPaymentOnboarding) as Arc<dyn PaymentProviderOnboarding>,
            ],
        ));
        assert_eq!(
            chaos_application::ports::PaymentWebhookConfigurationRepository::webhook_secret_references(
                payment_repository.as_ref(),
                "testpay",
                &provider_account_reference,
            )
            .await
            .unwrap()
            .iter()
            .map(|reference| reference.expose_reference())
            .collect::<Vec<_>>(),
            vec!["test://webhook"]
        );
        assert!(
            chaos_application::ports::PaymentWebhookConfigurationRepository::webhook_secret_references(
                payment_repository.as_ref(),
                "testpay",
                "unknown-store",
            )
            .await
            .unwrap()
            .is_empty()
        );
        let payment_workers = PaymentWorkers::new(
            payment_repository.clone(),
            payment_repository.clone(),
            payment_repository.clone(),
            Vec::new(),
            vec![Arc::new(SandboxPaymentProvider) as Arc<dyn PaymentProviderOnboarding>],
        );
        let fulfillment_workers = Arc::new(FulfillmentWorkers::new(
            Arc::new(PostgresFulfillmentRepository::new(runtime_pool.clone())),
            shipping_repository.clone(),
            [shipping_provider],
        ));
        let app = router(state);

        let analytics_policy_uri =
            format!("/admin/v1/stores/{}/analytics-policy", store_id.as_uuid());
        let default_analytics_policy = app
            .clone()
            .oneshot(request(Method::GET, &analytics_policy_uri, None, None))
            .await
            .unwrap();
        assert_eq!(default_analytics_policy.status(), StatusCode::OK);
        let default_analytics_policy = response_json(default_analytics_policy).await;
        assert_eq!(
            default_analytics_policy["data"]["policy_version"],
            "builtin-v1"
        );
        assert_eq!(
            default_analytics_policy["data"]["raw_event_retention_days"],
            30
        );
        assert_eq!(
            default_analytics_policy["data"]["advertising_exports_enabled"],
            false
        );
        let analytics_destinations_uri = format!(
            "/admin/v1/stores/{}/analytics-destinations",
            store_id.as_uuid()
        );
        for (provider, reference, source_url, secret_reference) in [
            (
                "meta_capi",
                "123456789",
                Some("https://store.example/"),
                "env://CHAOS_ANALYTICS_SECRET_META_TEST",
            ),
            (
                "ga4",
                "G-ABC123",
                None,
                "env://CHAOS_ANALYTICS_SECRET_GA4_TEST",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    Method::PUT,
                    &analytics_destinations_uri,
                    Some(&format!("analytics-destination-{provider}")),
                    Some(json!({
                        "provider": provider,
                        "external_destination_reference": reference,
                        "event_source_base_url": source_url,
                        "credential_secret_reference": secret_reference,
                        "enabled": true
                    })),
                ))
                .await
                .unwrap();
            let status = response.status();
            let response = response_json(response).await;
            assert_eq!(status, StatusCode::OK, "{response}");
            assert_eq!(response["data"]["provider"], provider);
            assert_eq!(response["data"]["credentials_configured"], true);
            assert!(
                response["data"]
                    .get("credential_secret_reference")
                    .is_none()
            );
        }
        let destinations = app
            .clone()
            .oneshot(request(
                Method::GET,
                &analytics_destinations_uri,
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(destinations.status(), StatusCode::OK);
        assert_eq!(
            response_json(destinations).await["data"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let analytics_event_id = Uuid::now_v7();
        let analytics_anonymous_id = Uuid::now_v7();
        let analytics_session_id = Uuid::now_v7();
        let analytics_occurred_at = test_clock
            .now()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let analytics_body = json!({
            "events": [
                {
                    "event_id": analytics_event_id,
                    "event_name": "page_viewed",
                    "schema_version": 1,
                    "occurred_at": analytics_occurred_at,
                    "anonymous_id": analytics_anonymous_id,
                    "session_id": analytics_session_id,
                    "consent": {
                        "analytics_storage": true,
                        "advertising_storage": false,
                        "policy_version": "cmp-v1"
                    },
                    "properties": {
                        "path": "/products",
                        "title": "Products",
                        "referrer_domain": "search.example",
                        "campaign_source": "newsletter",
                        "campaign_medium": "email",
                        "campaign_name": "launch"
                    }
                },
                {
                    "event_id": Uuid::now_v7(),
                    "event_name": "engagement_heartbeat",
                    "schema_version": 1,
                    "occurred_at": analytics_occurred_at,
                    "anonymous_id": analytics_anonymous_id,
                    "session_id": analytics_session_id,
                    "consent": {
                        "analytics_storage": false,
                        "advertising_storage": false,
                        "policy_version": "cmp-v1"
                    },
                    "properties": {
                        "page_view_event_id": analytics_event_id,
                        "active_milliseconds": 60000
                    }
                }
            ]
        });
        let analytics = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(analytics_body.clone()),
            ))
            .await
            .unwrap();
        let analytics_status = analytics.status();
        let analytics = response_json(analytics).await;
        assert_eq!(analytics_status, StatusCode::OK, "{analytics:?}");
        assert_eq!(analytics["data"]["received"], 2);
        assert_eq!(analytics["data"]["stored"], 1);
        assert_eq!(analytics["data"]["discarded_for_consent"], 1);
        let attribution_input: (String, String, String, String, String) = sqlx::query_as(
            "SELECT landing_path, referrer_domain, campaign_source, campaign_medium, campaign_name \
             FROM analytics.behavior_events WHERE store_id = $2 \
               AND event_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(analytics_event_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            attribution_input,
            (
                "/products".into(),
                "search.example".into(),
                "newsletter".into(),
                "email".into(),
                "launch".into(),
            )
        );
        let analytics_replay = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(analytics_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(analytics_replay.status(), StatusCode::OK);
        assert_eq!(
            response_json(analytics_replay).await["data"]["duplicates"],
            1
        );
        let analytics_wrong_scope = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(catalog_secret),
                None,
                Some(analytics_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(analytics_wrong_scope.status(), StatusCode::FORBIDDEN);
        let invalid_analytics = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(json!({
                    "events": [{
                        "event_id": Uuid::now_v7(),
                        "event_name": "engagement_heartbeat",
                        "schema_version": 1,
                        "occurred_at": analytics_occurred_at,
                        "anonymous_id": analytics_anonymous_id,
                        "session_id": analytics_session_id,
                        "consent": {
                            "analytics_storage": true,
                            "advertising_storage": false,
                            "policy_version": "cmp-v1"
                        },
                        "properties": {
                            "page_view_event_id": analytics_event_id,
                            "active_milliseconds": 60001
                        }
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(invalid_analytics.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let other_store_analytics = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(other_store_secret),
                None,
                Some(analytics_body),
            ))
            .await
            .unwrap();
        assert_eq!(other_store_analytics.status(), StatusCode::OK);
        assert_eq!(
            response_json(other_store_analytics).await["data"]["stored"],
            1
        );
        let session_anonymous_id = Uuid::now_v7();
        let session_client_id = Uuid::now_v7();
        let session_page_event_id = Uuid::now_v7();
        let session_started_at = test_clock.now() - time::Duration::minutes(70);
        let session_last_event_at = test_clock.now() - time::Duration::minutes(39);
        let session_bridge_at = test_clock.now() - time::Duration::minutes(55);
        let session_heartbeat_at = test_clock.now() - time::Duration::minutes(54);
        let format_time = |value: time::OffsetDateTime| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        };
        let sessionization_response = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(json!({
                    "events": [
                        {
                            "event_id": session_page_event_id,
                            "event_name": "page_viewed",
                            "schema_version": 1,
                            "occurred_at": format_time(session_started_at),
                            "anonymous_id": session_anonymous_id,
                            "session_id": session_client_id,
                            "consent": {
                                "analytics_storage": true,
                                "advertising_storage": false,
                                "policy_version": "cmp-v1"
                            },
                            "properties": {"path": "/session/start"}
                        },
                        {
                            "event_id": Uuid::now_v7(),
                            "event_name": "page_viewed",
                            "schema_version": 1,
                            "occurred_at": format_time(session_last_event_at),
                            "anonymous_id": session_anonymous_id,
                            "session_id": session_client_id,
                            "consent": {
                                "analytics_storage": true,
                                "advertising_storage": false,
                                "policy_version": "cmp-v1"
                            },
                            "properties": {"path": "/session/end"}
                        },
                        {
                            "event_id": Uuid::now_v7(),
                            "event_name": "page_viewed",
                            "schema_version": 1,
                            "occurred_at": format_time(session_bridge_at),
                            "anonymous_id": session_anonymous_id,
                            "session_id": session_client_id,
                            "consent": {
                                "analytics_storage": true,
                                "advertising_storage": false,
                                "policy_version": "cmp-v1"
                            },
                            "properties": {"path": "/session/bridge"}
                        },
                        {
                            "event_id": Uuid::now_v7(),
                            "event_name": "engagement_heartbeat",
                            "schema_version": 1,
                            "occurred_at": format_time(session_heartbeat_at),
                            "anonymous_id": session_anonymous_id,
                            "session_id": session_client_id,
                            "consent": {
                                "analytics_storage": true,
                                "advertising_storage": false,
                                "policy_version": "cmp-v1"
                            },
                            "properties": {
                                "page_view_event_id": session_page_event_id,
                                "active_milliseconds": 60000
                            }
                        }
                    ]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(sessionization_response.status(), StatusCode::OK);
        assert_eq!(
            response_json(sessionization_response).await["data"]["stored"],
            4
        );

        let abandoned_worker_id = Uuid::now_v7();
        let abandoned_jobs = analytics_repository
            .claim_sessionization(
                abandoned_worker_id,
                100,
                test_clock.now(),
                test_clock.now() - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert!(abandoned_jobs.len() >= 6);
        let recovered_worker_id = Uuid::now_v7();
        assert_eq!(
            analytics_workers
                .run_sessionization_batch(
                    recovered_worker_id,
                    test_clock.now() + time::Duration::minutes(2),
                    100,
                )
                .await
                .unwrap(),
            abandoned_jobs.len()
        );
        let stale_result = analytics_repository
            .finish_sessionization(
                abandoned_worker_id,
                &abandoned_jobs[0],
                Ok(SessionEventContribution::from_event(
                    abandoned_jobs[0].event_name,
                    abandoned_jobs[0].active_engagement_milliseconds,
                )
                .unwrap()),
                test_clock.now() + time::Duration::minutes(2),
            )
            .await;
        assert!(matches!(
            stale_result,
            Err(ApplicationError::Conflict { .. })
        ));

        let session: (i64, i64, i64, time::OffsetDateTime, time::OffsetDateTime) = sqlx::query_as(
            "SELECT event_count, page_view_count, active_engagement_milliseconds, \
                        started_at, last_event_at \
                 FROM analytics.sessions \
                 WHERE store_id = $2 \
                   AND anonymous_id = $3 AND client_session_id = $4",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(session_anonymous_id)
        .bind(session_client_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!((session.0, session.1, session.2), (4, 3, 60_000));
        assert_eq!(session.3, session_started_at);
        assert_eq!(session.4, session_last_event_at);
        let sessionization_metrics: (i64, i64, i64, f64) =
            sqlx::query_as("SELECT * FROM analytics.sessionization_metrics()")
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        assert_eq!(sessionization_metrics.0, 0);
        assert_eq!(sessionization_metrics.1, 0);
        assert_eq!(sessionization_metrics.2, 0);

        let disabled_policy_body = json!({
            "behavior_collection_enabled": false,
            "advertising_exports_enabled": false,
            "identity_linking_enabled": false,
            "raw_event_retention_days": 7
        });
        let disabled_policy = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &analytics_policy_uri,
                Some("disable-analytics-collection"),
                Some(disabled_policy_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(disabled_policy.status(), StatusCode::OK);
        assert_eq!(
            response_json(disabled_policy).await["data"]["policy_version"],
            "store-v1"
        );
        let disabled_policy_replay = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &analytics_policy_uri,
                Some("disable-analytics-collection"),
                Some(disabled_policy_body),
            ))
            .await
            .unwrap();
        assert_eq!(disabled_policy_replay.status(), StatusCode::OK);
        assert_eq!(
            response_json(disabled_policy_replay).await["data"]["policy_version"],
            "store-v1"
        );
        let policy_event_id = Uuid::now_v7();
        let policy_event = |event_id| {
            json!({
                "events": [{
                    "event_id": event_id,
                    "event_name": "page_viewed",
                    "schema_version": 1,
                    "occurred_at": analytics_occurred_at,
                    "anonymous_id": Uuid::now_v7(),
                    "session_id": Uuid::now_v7(),
                    "consent": {
                        "analytics_storage": true,
                        "advertising_storage": true,
                        "policy_version": "cmp-v1"
                    },
                    "properties": {"path": "/policy"}
                }]
            })
        };
        let policy_discard = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(policy_event(policy_event_id)),
            ))
            .await
            .unwrap();
        assert_eq!(policy_discard.status(), StatusCode::OK);
        let policy_discard = response_json(policy_discard).await;
        assert_eq!(policy_discard["data"]["stored"], 0);
        assert_eq!(policy_discard["data"]["discarded_for_policy"], 1);
        assert_eq!(
            policy_discard["data"]["collection_policy_version"],
            "store-v1"
        );
        let enabled_policy = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &analytics_policy_uri,
                Some("enable-analytics-collection"),
                Some(json!({
                    "behavior_collection_enabled": true,
                    "advertising_exports_enabled": true,
                    "identity_linking_enabled": false,
                    "raw_event_retention_days": 7
                })),
            ))
            .await
            .unwrap();
        assert_eq!(enabled_policy.status(), StatusCode::OK);
        assert_eq!(
            response_json(enabled_policy).await["data"]["policy_version"],
            "store-v2"
        );
        let invalid_policy = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &analytics_policy_uri,
                Some("invalid-analytics-retention"),
                Some(json!({
                    "behavior_collection_enabled": true,
                    "advertising_exports_enabled": false,
                    "identity_linking_enabled": false,
                    "raw_event_retention_days": 0
                })),
            ))
            .await
            .unwrap();
        assert_eq!(invalid_policy.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let retained_policy_event_id = Uuid::now_v7();
        let retained_policy_event = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(policy_event(retained_policy_event_id)),
            ))
            .await
            .unwrap();
        assert_eq!(retained_policy_event.status(), StatusCode::OK);
        let retained_policy_event = response_json(retained_policy_event).await;
        assert_eq!(retained_policy_event["data"]["stored"], 1);
        assert_eq!(
            retained_policy_event["data"]["collection_policy_version"],
            "store-v2"
        );
        let retention_window: (time::OffsetDateTime, time::OffsetDateTime) = sqlx::query_as(
            "SELECT received_at, retention_expires_at FROM analytics.behavior_events \
             WHERE store_id = $2 AND event_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(retained_policy_event_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            retention_window.1 - retention_window.0,
            time::Duration::days(7)
        );
        let store_policy = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("/admin/v1/stores/{}/analytics-policy", store_id.as_uuid()),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(store_policy.status(), StatusCode::OK);
        let analytics_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM analytics.behavior_events WHERE event_id = $1",
        )
        .bind(analytics_event_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(analytics_rows, 2);
        let isolated_store_id = StoreId::new();
        let isolated_channel_id = SalesChannelId::new();
        sqlx::query(
            "INSERT INTO merchant.stores (id, code, name, status) \
             VALUES ($1, $2, 'isolated', 'active')",
        )
        .bind(isolated_store_id.as_uuid())
        .bind(format!("isolated-{}", &suffix[12..28]))
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, 'web', 'Web', 'web', true)",
        )
        .bind(isolated_channel_id.as_uuid())
        .bind(isolated_store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        let isolated_behavior_event_id: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.behavior_events \
             (id, event_id, store_id, sales_channel_id, event_name, \
              schema_version, source, anonymous_id, session_id, analytics_storage_consent, \
              advertising_storage_consent, advertising_export_eligible, consent_policy_version, collection_policy_version, \
              properties, landing_path, occurred_at, received_at, retention_expires_at) \
             VALUES (uuidv7(),uuidv7(),$1,$2,'page_viewed',1,'browser',uuidv7(),uuidv7(), \
                     true,false,false,'cmp-v1','builtin-v1','{\"path\":\"/\"}'::jsonb, \
                     '/',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP + INTERVAL '30 days') \
             RETURNING id",
        )
        .bind(isolated_store_id.as_uuid())
        .bind(isolated_channel_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        let mut analytics_rls = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *analytics_rls)
            .await
            .unwrap();
        let visible_analytics_rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.behavior_events")
                .fetch_one(&mut *analytics_rls)
                .await
                .unwrap();
        assert_eq!(visible_analytics_rows, 6);
        let visible_analytics_sessions: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.sessions")
                .fetch_one(&mut *analytics_rls)
                .await
                .unwrap();
        assert_eq!(visible_analytics_sessions, 2);
        let cross_store_retention_update: (i64, i64) =
            sqlx::query_as("SELECT * FROM analytics.apply_store_retention_policy($1,1)")
                .bind(isolated_store_id.as_uuid())
                .fetch_one(&mut *analytics_rls)
                .await
                .unwrap();
        assert_eq!(cross_store_retention_update, (0, 0));
        analytics_rls.rollback().await.unwrap();

        let retention_purge = analytics_workers
            .run_retention_batch(test_clock.now() + time::Duration::days(8), 1000)
            .await
            .unwrap();
        assert_eq!(retention_purge.behavior_events_deleted, 6);
        assert_eq!(retention_purge.sessions_deleted, 2);
        let retained_main_store_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM analytics.behavior_events \
             WHERE store_id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(retained_main_store_events, 0);
        let retained_other_store_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM analytics.behavior_events \
             WHERE store_id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(other_store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(retained_other_store_events, 1);
        let isolated_retention_days: f64 = sqlx::query_scalar(
            "SELECT (extract(epoch FROM retention_expires_at - received_at) / 86400)::double precision \
             FROM analytics.behavior_events WHERE id = $1",
        )
        .bind(isolated_behavior_event_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(isolated_retention_days, 30.0);

        let provider_accounts_uri = format!(
            "/admin/v1/stores/{}/payment-provider-accounts",
            store_id.as_uuid()
        );
        let ready_testpay = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &format!("{provider_accounts_uri}/{testpay_provider_account_id}"),
                Some("verify-testpay-readiness"),
                Some(json!({
                    "display_name": "TestPay",
                    "credential_secret_reference": "test://credential",
                    "webhook_secret_reference": "test://webhook",
                    "enabled": true
                })),
            ))
            .await
            .unwrap();
        assert_eq!(ready_testpay.status(), StatusCode::OK);
        let ready_testpay = response_json(ready_testpay).await;
        assert_eq!(ready_testpay["data"]["readiness_status"], "ready");
        assert!(ready_testpay["data"]["readiness_checked_at"].is_string());
        assert!(ready_testpay["data"]["readiness_valid_until"].is_string());
        let first_reconciliation_at = test_clock.now() + time::Duration::minutes(1);
        sqlx::query(
            "UPDATE payments.provider_accounts SET readiness_reconcile_at = $2 WHERE id = $1",
        )
        .bind(testpay_provider_account_id)
        .bind(first_reconciliation_at)
        .execute(&owner_pool)
        .await
        .unwrap();
        let failing_readiness_workers = PaymentWorkers::new(
            payment_repository.clone(),
            payment_repository.clone(),
            payment_repository.clone(),
            Vec::new(),
            vec![Arc::new(UnavailableTestpayOnboarding) as Arc<dyn PaymentProviderOnboarding>],
        );
        assert_eq!(
            failing_readiness_workers
                .run_readiness_batch(Uuid::now_v7(), first_reconciliation_at, 10)
                .await
                .unwrap(),
            1
        );
        let failed_reconciliation: (bool, i32, String, time::OffsetDateTime) = sqlx::query_as(
            "SELECT enabled, readiness_reconcile_attempts, readiness_last_error, \
                    readiness_reconcile_at FROM payments.provider_accounts WHERE id = $1",
        )
        .bind(testpay_provider_account_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert!(failed_reconciliation.0);
        assert_eq!(failed_reconciliation.1, 1);
        assert_eq!(failed_reconciliation.2, "dependency testpay is unavailable");
        assert_eq!(
            failed_reconciliation.3,
            first_reconciliation_at + time::Duration::seconds(1)
        );
        let readiness_metrics: (i64, i64, i64, i64) =
            sqlx::query_as("SELECT * FROM payments.provider_readiness_metrics()")
                .fetch_one(&runtime_pool)
                .await
                .unwrap();
        assert_eq!(readiness_metrics.1, 1);
        assert_eq!(
            payment_workers
                .run_readiness_batch(Uuid::now_v7(), failed_reconciliation.3, 10)
                .await
                .unwrap(),
            1
        );
        let abandoned_at = first_reconciliation_at + time::Duration::minutes(1);
        sqlx::query(
            "UPDATE payments.provider_accounts SET readiness_reconcile_at = $2 WHERE id = $1",
        )
        .bind(testpay_provider_account_id)
        .bind(abandoned_at)
        .execute(&owner_pool)
        .await
        .unwrap();
        let abandoned_worker = Uuid::now_v7();
        let abandoned = PaymentProviderReadinessQueue::claim_provider_readiness(
            payment_repository.as_ref(),
            abandoned_worker,
            10,
            abandoned_at,
            abandoned_at - time::Duration::minutes(1),
        )
        .await
        .unwrap();
        assert_eq!(abandoned.len(), 1);
        assert_eq!(
            payment_workers
                .run_readiness_batch(
                    Uuid::now_v7(),
                    abandoned_at + time::Duration::minutes(2),
                    10,
                )
                .await
                .unwrap(),
            1
        );
        assert!(
            PaymentProviderReadinessQueue::finish_provider_readiness(
                payment_repository.as_ref(),
                abandoned_worker,
                abandoned[0].provider_account_id,
                Err("former worker".into()),
                abandoned_at + time::Duration::minutes(2),
            )
            .await
            .is_err()
        );
        let expired_at = abandoned_at + time::Duration::hours(25);
        sqlx::query(
            "UPDATE payments.provider_accounts \
             SET readiness_valid_until = $2, readiness_reconcile_at = $2 WHERE id = $1",
        )
        .bind(testpay_provider_account_id)
        .bind(expired_at - time::Duration::seconds(1))
        .execute(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            payment_workers
                .run_readiness_batch(Uuid::now_v7(), expired_at, 10)
                .await
                .unwrap(),
            0
        );
        let expired_provider = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{provider_accounts_uri}/{testpay_provider_account_id}"),
                None,
                None,
            ))
            .await
            .unwrap();
        let expired_provider = response_json(expired_provider).await;
        assert_eq!(expired_provider["data"]["enabled"], false);
        assert_eq!(
            expired_provider["data"]["readiness_status"],
            "action_required"
        );
        assert_eq!(
            expired_provider["data"]["readiness_blocker_codes"],
            json!(["readiness_expired"])
        );
        let restored_testpay = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &format!("{provider_accounts_uri}/{testpay_provider_account_id}"),
                Some("restore-testpay-readiness"),
                Some(json!({
                    "display_name": "TestPay",
                    "credential_secret_reference": "test://credential",
                    "webhook_secret_reference": "test://webhook",
                    "enabled": true
                })),
            ))
            .await
            .unwrap();
        assert_eq!(restored_testpay.status(), StatusCode::OK);
        assert_eq!(
            response_json(restored_testpay).await["data"]["readiness_status"],
            "ready"
        );
        let blocked_provider = app
            .clone()
            .oneshot(request(
                Method::POST,
                &provider_accounts_uri,
                Some("create-blocked-provider"),
                Some(json!({
                    "provider": "blockedpay",
                    "display_name": "Blocked Provider",
                    "external_account_reference": format!("blocked-store-{suffix}"),
                    "credential_secret_reference": "test://blocked-credential",
                    "webhook_secret_reference": "test://blocked-webhook",
                    "enabled": true
                })),
            ))
            .await
            .unwrap();
        assert_eq!(blocked_provider.status(), StatusCode::CREATED);
        let blocked_provider = response_json(blocked_provider).await;
        assert_eq!(blocked_provider["data"]["enabled"], false);
        assert_eq!(
            blocked_provider["data"]["readiness_status"],
            "action_required"
        );
        assert_eq!(
            blocked_provider["data"]["readiness_blocker_codes"],
            json!(["account_incomplete"])
        );
        let stripe_configuration = json!({
            "provider": "stripe",
            "display_name": "Stripe Live",
            "external_account_reference": format!("acct_stripe_{suffix}"),
            "credential_secret_reference": "enc://cGF5bWVudHMtc3RyaXBlLWFwaS1rZXk",
            "webhook_secret_reference": "enc://cGF5bWVudHMtc3RyaXBlLXdlYmhvb2stc2VjcmV0",
            "enabled": false
        });
        let stripe_account = app
            .clone()
            .oneshot(request(
                Method::POST,
                &provider_accounts_uri,
                Some("create-stripe-provider"),
                Some(stripe_configuration.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(stripe_account.status(), StatusCode::CREATED);
        let stripe_account = response_json(stripe_account).await;
        let stripe_account_id = stripe_account["data"]["id"].as_str().unwrap().to_owned();
        assert_eq!(stripe_account["data"]["provider"], "stripe");
        assert_eq!(stripe_account["data"]["enabled"], false);
        assert_eq!(stripe_account["data"]["readiness_status"], "unchecked");
        assert!(stripe_account["data"]["readiness_checked_at"].is_null());
        assert_eq!(stripe_account["data"]["credentials_configured"], true);
        assert!(stripe_account["data"]["credential_secret_reference"].is_null());
        assert!(stripe_account["data"]["webhook_secret_reference"].is_null());

        let duplicate_provider = app
            .clone()
            .oneshot(request(
                Method::POST,
                &provider_accounts_uri,
                Some("duplicate-stripe-provider"),
                Some(stripe_configuration),
            ))
            .await
            .unwrap();
        assert_eq!(duplicate_provider.status(), StatusCode::CONFLICT);
        let provider_page = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{provider_accounts_uri}?limit=10"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(provider_page.status(), StatusCode::OK);
        assert_eq!(
            response_json(provider_page).await["data"]
                .as_array()
                .unwrap()
                .len(),
            3
        );

        let stripe_account_uri = format!("{provider_accounts_uri}/{stripe_account_id}");
        let disabled_stripe = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &stripe_account_uri,
                Some("disable-stripe-provider"),
                Some(json!({
                    "display_name": "Stripe",
                    "credential_secret_reference": "enc://cGF5bWVudHMtc3RyaXBlLWFwaS1rZXktdjI",
                    "webhook_secret_reference": "enc://cGF5bWVudHMtc3RyaXBlLXdlYmhvb2stc2VjcmV0LXYy",
                    "enabled": false
                })),
            ))
            .await
            .unwrap();
        assert_eq!(disabled_stripe.status(), StatusCode::OK);
        let disabled_stripe = response_json(disabled_stripe).await;
        assert_eq!(disabled_stripe["data"]["enabled"], false);
        assert!(disabled_stripe["data"]["credential_rotation_expires_at"].is_string());
        assert!(disabled_stripe["data"]["webhook_rotation_expires_at"].is_string());
        assert_eq!(
            chaos_application::ports::PaymentWebhookConfigurationRepository::webhook_secret_references(
                payment_repository.as_ref(),
                "stripe",
                &format!("acct_stripe_{suffix}"),
            )
            .await
            .unwrap()
            .iter()
            .map(|reference| reference.expose_reference())
            .collect::<Vec<_>>(),
            vec![
                "enc://cGF5bWVudHMtc3RyaXBlLXdlYmhvb2stc2VjcmV0LXYy",
                "enc://cGF5bWVudHMtc3RyaXBlLXdlYmhvb2stc2VjcmV0"
            ]
        );

        let rotation_expiry: (time::OffsetDateTime, time::OffsetDateTime) = sqlx::query_as(
            "SELECT credential_rotation_expires_at, webhook_rotation_expires_at \
             FROM payments.provider_accounts WHERE id = $1",
        )
        .bind(Uuid::parse_str(&stripe_account_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        let unchanged_rotation = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &stripe_account_uri,
                Some("retain-stripe-rotation-window"),
                Some(json!({
                    "display_name": "Stripe",
                    "credential_secret_reference": "enc://cGF5bWVudHMtc3RyaXBlLWFwaS1rZXktdjI",
                    "webhook_secret_reference": "enc://cGF5bWVudHMtc3RyaXBlLXdlYmhvb2stc2VjcmV0LXYy",
                    "enabled": false
                })),
            ))
            .await
            .unwrap();
        assert_eq!(unchanged_rotation.status(), StatusCode::OK);
        assert_eq!(
            sqlx::query_as::<_, (time::OffsetDateTime, time::OffsetDateTime)>(
                "SELECT credential_rotation_expires_at, webhook_rotation_expires_at \
                 FROM payments.provider_accounts WHERE id = $1",
            )
            .bind(Uuid::parse_str(&stripe_account_id).unwrap())
            .fetch_one(&owner_pool)
            .await
            .unwrap(),
            rotation_expiry
        );
        sqlx::query(
            "UPDATE payments.provider_accounts \
             SET webhook_rotation_expires_at = CURRENT_TIMESTAMP - INTERVAL '1 second' \
             WHERE id = $1",
        )
        .bind(Uuid::parse_str(&stripe_account_id).unwrap())
        .execute(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            chaos_application::ports::PaymentWebhookConfigurationRepository::webhook_secret_references(
                payment_repository.as_ref(),
                "stripe",
                &format!("acct_stripe_{suffix}"),
            )
            .await
            .unwrap()
            .iter()
            .map(|reference| reference.expose_reference())
            .collect::<Vec<_>>(),
            vec!["enc://cGF5bWVudHMtc3RyaXBlLXdlYmhvb2stc2VjcmV0LXYy"]
        );

        let cross_store_provider = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!(
                    "/admin/v1/stores/{}/payment-provider-accounts/{stripe_account_id}",
                    other_store_id.as_uuid()
                ),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(cross_store_provider.status(), StatusCode::FORBIDDEN);

        let unauthorized = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/carts",
                None,
                Some("u"),
                Some(json!({})),
            ))
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let forbidden = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/carts",
                Some(catalog_secret),
                Some("f"),
                Some(json!({})),
            ))
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let shopper_session = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/shopper-sessions",
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(shopper_session.status(), StatusCode::CREATED);
        let shopper_session = response_json(shopper_session).await;
        let shopper_token = shopper_session["data"]["shopper_token"]
            .as_str()
            .unwrap()
            .to_owned();
        let missing_idempotency = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    "/store/v1/carts",
                    Some(full_secret),
                    None,
                    Some(json!({})),
                ),
                &shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(
            missing_idempotency.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let missing = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                &format!("/store/v1/carts/{}", Uuid::now_v7()),
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let create = with_shopper_token(
            store_request(
                Method::POST,
                "/store/v1/carts",
                Some(full_secret),
                Some("create"),
                Some(json!({})),
            ),
            &shopper_token,
        );
        let created = app.clone().oneshot(create).await.unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = response_json(created).await;
        let cart_id = created["data"]["id"].as_str().unwrap();
        assert_eq!(created["data"]["shopper_token"], shopper_token);
        let shopper_token = shopper_token.as_str();
        let unrelated_session = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/shopper-sessions",
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        let unrelated_session = response_json(unrelated_session).await;
        let unrelated_token = unrelated_session["data"]["shopper_token"].as_str().unwrap();
        let cross_shopper = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/carts/{cart_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                unrelated_token,
            ))
            .await
            .unwrap();
        assert_eq!(cross_shopper.status(), StatusCode::NOT_FOUND);

        let line_uri = format!("/store/v1/carts/{cart_id}/lines/{}", variant_id.as_uuid());
        let invalid_line = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::PUT,
                    &line_uri,
                    Some(full_secret),
                    Some("invalid-line"),
                    Some(json!({"quantity": 0})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_line.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let updated = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::PUT,
                    &line_uri,
                    Some(full_secret),
                    Some("line"),
                    Some(json!({"quantity": 2})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let updated = response_json(updated).await;
        assert_eq!(updated["data"]["subtotal_amount_minor"], 2500);

        let associated = app
            .clone()
            .oneshot(with_customer_session(with_shopper_token(
                store_request(
                    Method::POST,
                    "/store/v1/customer/associate",
                    Some(full_secret),
                    Some("associate-customer"),
                    None,
                ),
                shopper_token,
            )))
            .await
            .unwrap();
        let associated_status = associated.status();
        let associated = response_json(associated).await;
        assert_eq!(associated_status, StatusCode::CREATED, "{associated}");
        let customer_id = associated["data"]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            associated["data"]["email"],
            format!("sales-http-{suffix}@example.com")
        );

        let identity_anonymous_id = Uuid::now_v7();
        let identity_session_id = Uuid::now_v7();
        let identity_link_body = json!({
            "anonymous_id": identity_anonymous_id,
            "consent": {
                "analytics_storage": true,
                "advertising_storage": false,
                "policy_version": "cmp-v2"
            }
        });
        let identity_link_disabled = app
            .clone()
            .oneshot(with_customer_session(store_request(
                Method::POST,
                "/store/v1/analytics/identity-links",
                Some(full_secret),
                Some("identity-link-disabled"),
                Some(identity_link_body.clone()),
            )))
            .await
            .unwrap();
        assert_eq!(identity_link_disabled.status(), StatusCode::CONFLICT);
        let identity_policy = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &analytics_policy_uri,
                Some("enable-analytics-identity-linking"),
                Some(json!({
                    "behavior_collection_enabled": true,
                    "advertising_exports_enabled": false,
                    "identity_linking_enabled": true,
                    "raw_event_retention_days": 7
                })),
            ))
            .await
            .unwrap();
        assert_eq!(identity_policy.status(), StatusCode::OK);
        assert_eq!(
            response_json(identity_policy).await["data"]["policy_version"],
            "store-v3"
        );
        let identity_event_id = Uuid::now_v7();
        let identity_event = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(json!({
                    "events": [{
                        "event_id": identity_event_id,
                        "event_name": "page_viewed",
                        "schema_version": 1,
                        "occurred_at": test_clock.now()
                            .format(&time::format_description::well_known::Rfc3339)
                            .unwrap(),
                        "anonymous_id": identity_anonymous_id,
                        "session_id": identity_session_id,
                        "consent": {
                            "analytics_storage": true,
                            "advertising_storage": false,
                            "policy_version": "cmp-v2"
                        },
                        "properties": {"path": "/identified"}
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(identity_event.status(), StatusCode::OK);
        assert_eq!(response_json(identity_event).await["data"]["stored"], 1);
        assert_eq!(
            analytics_workers
                .run_sessionization_batch(Uuid::now_v7(), test_clock.now(), 100)
                .await
                .unwrap(),
            1
        );
        let identity_link_without_consent = app
            .clone()
            .oneshot(with_customer_session(store_request(
                Method::POST,
                "/store/v1/analytics/identity-links",
                Some(full_secret),
                Some("identity-link-no-consent"),
                Some(json!({
                    "anonymous_id": Uuid::now_v7(),
                    "consent": {
                        "analytics_storage": false,
                        "advertising_storage": false,
                        "policy_version": "cmp-v2"
                    }
                })),
            )))
            .await
            .unwrap();
        assert_eq!(
            identity_link_without_consent.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let identity_link = app
            .clone()
            .oneshot(with_customer_session(store_request(
                Method::POST,
                "/store/v1/analytics/identity-links",
                Some(full_secret),
                Some("identity-link"),
                Some(identity_link_body.clone()),
            )))
            .await
            .unwrap();
        assert_eq!(identity_link.status(), StatusCode::CREATED);
        let identity_link = response_json(identity_link).await;
        assert_eq!(
            identity_link["data"]["anonymous_id"],
            identity_anonymous_id.to_string()
        );
        assert_eq!(
            identity_link["data"]["collection_policy_version"],
            "store-v3"
        );
        assert!(identity_link["data"].get("customer_id").is_none());
        let identity_link_replay = app
            .clone()
            .oneshot(with_customer_session(store_request(
                Method::POST,
                "/store/v1/analytics/identity-links",
                Some(full_secret),
                Some("identity-link"),
                Some(identity_link_body),
            )))
            .await
            .unwrap();
        assert_eq!(identity_link_replay.status(), StatusCode::CREATED);
        assert_eq!(
            response_json(identity_link_replay).await["data"]["id"],
            identity_link["data"]["id"]
        );
        let stored_identity_customer_id: Uuid = sqlx::query_scalar(
            "SELECT customer_id FROM analytics.identity_links \
             WHERE store_id = $2 AND anonymous_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(identity_anonymous_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(stored_identity_customer_id.to_string(), customer_id);

        let erasure_uri = format!(
            "/admin/v1/stores/{}/analytics-erasure-requests",
            store_id.as_uuid()
        );
        let customer_erasure_body = json!({"customer_id": customer_id});
        let erasure = app
            .clone()
            .oneshot(request(
                Method::POST,
                &erasure_uri,
                Some("erase-customer-analytics"),
                Some(customer_erasure_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(erasure.status(), StatusCode::ACCEPTED);
        let erasure = response_json(erasure).await;
        let erasure_id = erasure["data"]["id"].as_str().unwrap();
        assert_eq!(erasure["data"]["status"], "pending");
        let erasure_replay = app
            .clone()
            .oneshot(request(
                Method::POST,
                &erasure_uri,
                Some("erase-customer-analytics"),
                Some(customer_erasure_body),
            ))
            .await
            .unwrap();
        assert_eq!(erasure_replay.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response_json(erasure_replay).await["data"]["id"],
            erasure_id
        );
        let invalid_erasure_selector = app
            .clone()
            .oneshot(request(
                Method::POST,
                &erasure_uri,
                Some("invalid-erasure-selector"),
                Some(json!({
                    "customer_id": customer_id,
                    "anonymous_id": identity_anonymous_id
                })),
            ))
            .await
            .unwrap();
        assert_eq!(
            invalid_erasure_selector.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            analytics_workers
                .run_erasure_batch(test_clock.now(), 100)
                .await
                .unwrap()
                .requests_completed,
            1
        );
        let completed_erasure = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{erasure_uri}/{erasure_id}"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(completed_erasure.status(), StatusCode::OK);
        let completed_erasure = response_json(completed_erasure).await;
        assert_eq!(completed_erasure["data"]["status"], "completed");
        assert_eq!(completed_erasure["data"]["behavior_events_deleted"], 1);
        assert_eq!(completed_erasure["data"]["sessions_deleted"], 1);
        assert_eq!(completed_erasure["data"]["identity_links_deleted"], 1);
        let retained_customer_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sales.customers \
             WHERE store_id = $2 AND id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(Uuid::parse_str(&customer_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(retained_customer_count, 1);
        let mut immutable_erasure_audit = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *immutable_erasure_audit)
            .await
            .unwrap();
        let erasure_mutation = sqlx::query(
            "UPDATE analytics.erasure_requests SET behavior_events_deleted = 0 WHERE id = $1",
        )
        .bind(Uuid::parse_str(erasure_id).unwrap())
        .execute(&mut *immutable_erasure_audit)
        .await;
        assert!(erasure_mutation.is_err());
        immutable_erasure_audit.rollback().await.unwrap();

        let direct_erasure_anonymous_id = Uuid::now_v7();
        let direct_erasure_event = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(json!({
                    "events": [{
                        "event_id": Uuid::now_v7(),
                        "event_name": "page_viewed",
                        "schema_version": 1,
                        "occurred_at": test_clock.now()
                            .format(&time::format_description::well_known::Rfc3339)
                            .unwrap(),
                        "anonymous_id": direct_erasure_anonymous_id,
                        "session_id": Uuid::now_v7(),
                        "consent": {
                            "analytics_storage": true,
                            "advertising_storage": false,
                            "policy_version": "cmp-v2"
                        },
                        "properties": {"path": "/anonymous-erasure"}
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(direct_erasure_event.status(), StatusCode::OK);
        assert_eq!(
            analytics_workers
                .run_sessionization_batch(Uuid::now_v7(), test_clock.now(), 100)
                .await
                .unwrap(),
            1
        );
        let direct_erasure = app
            .clone()
            .oneshot(request(
                Method::POST,
                &erasure_uri,
                Some("erase-anonymous-analytics"),
                Some(json!({"anonymous_id": direct_erasure_anonymous_id})),
            ))
            .await
            .unwrap();
        assert_eq!(direct_erasure.status(), StatusCode::ACCEPTED);
        let direct_erasure_id = response_json(direct_erasure).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            analytics_workers
                .run_erasure_batch(test_clock.now(), 100)
                .await
                .unwrap()
                .requests_completed,
            1
        );
        let completed_direct_erasure = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{erasure_uri}/{direct_erasure_id}"),
                None,
                None,
            ))
            .await
            .unwrap();
        let completed_direct_erasure = response_json(completed_direct_erasure).await;
        assert_eq!(
            completed_direct_erasure["data"]["behavior_events_deleted"],
            1
        );
        assert_eq!(completed_direct_erasure["data"]["sessions_deleted"], 1);
        assert_eq!(
            completed_direct_erasure["data"]["identity_links_deleted"],
            0
        );
        let surviving_other_store_analytics: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM analytics.behavior_events \
             WHERE store_id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(other_store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(surviving_other_store_analytics, 1);

        let missing_customer_session = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                "/store/v1/customer",
                Some(full_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(missing_customer_session.status(), StatusCode::UNAUTHORIZED);
        let other_store_customer = app
            .clone()
            .oneshot(with_customer_session(store_request(
                Method::GET,
                "/store/v1/customer",
                Some(other_store_secret),
                None,
                None,
            )))
            .await
            .unwrap();
        assert_eq!(other_store_customer.status(), StatusCode::NOT_FOUND);

        let address = json!({
            "label": "Home",
            "full_name": "Customer Buyer",
            "address_line1": "2 Market Street",
            "locality": "San Francisco",
            "administrative_area": "CA",
            "postal_code": "94105",
            "country_code": "US"
        });
        let saved_address_body = address;
        let saved_address = app
            .clone()
            .oneshot(with_customer_session(store_request(
                Method::POST,
                "/store/v1/customer/addresses",
                Some(full_secret),
                Some("save-customer-address"),
                Some(saved_address_body.clone()),
            )))
            .await
            .unwrap();
        assert_eq!(saved_address.status(), StatusCode::CREATED);
        assert_eq!(response_json(saved_address).await["data"]["label"], "Home");
        let duplicate_address = app
            .clone()
            .oneshot(with_customer_session(store_request(
                Method::POST,
                "/store/v1/customer/addresses",
                Some(full_secret),
                Some("duplicate-customer-address"),
                Some(saved_address_body),
            )))
            .await
            .unwrap();
        assert_eq!(duplicate_address.status(), StatusCode::CONFLICT);

        let quotes = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &format!("/store/v1/carts/{cart_id}/shipping-options"),
                    Some(full_secret),
                    None,
                    Some(json!({"destination_country": "us"})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(quotes.status(), StatusCode::OK);
        let quotes = response_json(quotes).await;
        assert_eq!(
            quotes["data"][0]["service_id"],
            shipping_service_id.as_uuid().to_string()
        );
        assert_eq!(quotes["data"][0]["amount_minor"], 500);

        let checkout_uri = format!("/store/v1/carts/{cart_id}/checkout");
        let address = json!({
            "full_name": "Guest Buyer",
            "address_line1": "1 Market Street",
            "locality": "San Francisco",
            "administrative_area": "CA",
            "postal_code": "94105",
            "country_code": "US"
        });
        let checkout_body = json!({
            "contact": {
                "email": " Guest@Example.COM ",
                "phone": "+14155552671"
            },
            "billing_address": address.clone(),
            "shipping_address": address.clone(),
            "shipping_service_id": shipping_service_id.as_uuid(),
            "promotion_code": "save300"
        });
        sqlx::query("UPDATE fulfillment.shipping_services SET status = 'archived' WHERE id = $1")
            .bind(shipping_service_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        let stale_shipping = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-stale-shipping"),
                    Some(checkout_body.clone()),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(stale_shipping.status(), StatusCode::UNPROCESSABLE_ENTITY);
        sqlx::query("UPDATE fulfillment.shipping_services SET status = 'active' WHERE id = $1")
            .bind(shipping_service_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("UPDATE pricing.tax_rules SET status = 'archived' WHERE id = $1")
            .bind(tax_rule_id)
            .execute(&owner_pool)
            .await
            .unwrap();
        let missing_tax_rule = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-missing-tax-rule"),
                    Some(checkout_body.clone()),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(missing_tax_rule.status(), StatusCode::UNPROCESSABLE_ENTITY);
        sqlx::query("UPDATE pricing.tax_rules SET status = 'active' WHERE id = $1")
            .bind(tax_rule_id)
            .execute(&owner_pool)
            .await
            .unwrap();
        let invalid_contact = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-invalid-contact"),
                    Some(json!({
                        "contact": {"email": "invalid"},
                        "billing_address": address.clone(),
                        "shipping_address": address.clone()
                    })),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_contact.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let missing_shipping = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-missing-shipping"),
                    Some(json!({
                        "contact": {"email": "guest@example.com"},
                        "billing_address": address.clone()
                    })),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(missing_shipping.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let mut invalid_promotion_body = checkout_body.clone();
        invalid_promotion_body["promotion_code"] = json!("NOT-A-CODE");
        let invalid_promotion = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-invalid-promotion"),
                    Some(invalid_promotion_body),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_promotion.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let checkout = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout"),
                    Some(checkout_body.clone()),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(checkout.status(), StatusCode::CREATED);
        let checkout = response_json(checkout).await;
        let checkout_id = checkout["data"]["id"].as_str().unwrap();
        assert_eq!(checkout["data"]["shipping_amount_minor"], 500);
        assert_eq!(checkout["data"]["discount_amount_minor"], 300);
        assert_eq!(checkout["data"]["tax_amount_minor"], 182);
        assert_eq!(checkout["data"]["tax_inclusive"], true);
        assert_eq!(checkout["data"]["tax_rule"]["rate_basis_points"], 900);
        assert_eq!(checkout["data"]["promotion"]["handle"], "save-three");
        assert_eq!(checkout["data"]["promotion"]["redemption_code"], "SAVE300");
        assert_eq!(checkout["data"]["total_amount_minor"], 2700);
        assert_eq!(
            checkout["data"]["lines"][0]["product_title"],
            "Sales Product"
        );
        assert!(checkout["data"]["inventory_reservation_id"].is_string());
        assert_eq!(checkout["data"]["contact"]["email"], "guest@example.com");
        assert_eq!(checkout["data"]["shipping_address"]["country_code"], "US");
        assert_eq!(checkout["data"]["customer_id"], customer_id);

        let attribution_policy = app
            .clone()
            .oneshot(request(
                Method::PUT,
                &analytics_policy_uri,
                Some("enable-analytics-attribution-exports"),
                Some(json!({
                    "behavior_collection_enabled": true,
                    "advertising_exports_enabled": true,
                    "identity_linking_enabled": true,
                    "raw_event_retention_days": 7
                })),
            ))
            .await
            .unwrap();
        assert_eq!(attribution_policy.status(), StatusCode::OK);
        assert_eq!(
            response_json(attribution_policy).await["data"]["policy_version"],
            "store-v4"
        );

        let attribution_anonymous_id = Uuid::now_v7();
        let attribution_session_id = Uuid::now_v7();
        let first_touch_event_id = Uuid::now_v7();
        let last_touch_event_id = Uuid::now_v7();
        let checkout_touch = app
            .clone()
            .oneshot(store_request(
                Method::POST,
                "/store/v1/analytics/events",
                Some(full_secret),
                None,
                Some(json!({
                    "events": [
                        {
                            "event_id": first_touch_event_id,
                            "event_name": "page_viewed",
                            "schema_version": 1,
                            "occurred_at": format_time(test_clock.now() - time::Duration::minutes(2)),
                            "anonymous_id": attribution_anonymous_id,
                            "session_id": attribution_session_id,
                            "consent": {
                                "analytics_storage": true,
                                "advertising_storage": true,
                                "policy_version": "cmp-attribution-v1"
                            },
                            "properties": {
                                "path": "/landing",
                                "campaign_source": "first-source",
                                "campaign_medium": "paid-social"
                            }
                        },
                        {
                            "event_id": last_touch_event_id,
                            "event_name": "page_viewed",
                            "schema_version": 1,
                            "occurred_at": format_time(test_clock.now() - time::Duration::minutes(1)),
                            "anonymous_id": attribution_anonymous_id,
                            "session_id": attribution_session_id,
                            "consent": {
                                "analytics_storage": true,
                                "advertising_storage": true,
                                "policy_version": "cmp-attribution-v1"
                            },
                            "properties": {
                                "path": "/checkout",
                                "campaign_source": "last-source",
                                "campaign_medium": "email"
                            }
                        },
                        {
                            "event_id": Uuid::now_v7(),
                            "event_name": "checkout_started",
                            "schema_version": 1,
                            "occurred_at": format_time(test_clock.now()),
                            "anonymous_id": attribution_anonymous_id,
                            "session_id": attribution_session_id,
                            "consent": {
                                "analytics_storage": true,
                                "advertising_storage": true,
                                "policy_version": "cmp-attribution-v1"
                            },
                            "properties": {
                                "cart_id": cart_id,
                                "checkout_id": checkout_id
                            }
                        }
                    ]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(checkout_touch.status(), StatusCode::OK);
        assert_eq!(response_json(checkout_touch).await["data"]["stored"], 3);

        let replay = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout"),
                    Some(checkout_body.clone()),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::CREATED);
        assert_eq!(response_json(replay).await["data"]["id"], checkout_id);

        let order_uri = format!("/store/v1/checkouts/{checkout_id}/order");
        let order = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &order_uri,
                    Some(full_secret),
                    Some("order"),
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(order.status(), StatusCode::CREATED);
        let order = response_json(order).await;
        let order_id = order["data"]["id"].as_str().unwrap();
        assert_eq!(order["data"]["status"], "pending");
        assert_eq!(order["data"]["shipping_amount_minor"], 500);
        assert_eq!(order["data"]["discount_amount_minor"], 300);
        assert_eq!(order["data"]["tax_amount_minor"], 182);
        assert_eq!(order["data"]["promotion"]["handle"], "save-three");
        assert_eq!(order["data"]["total_amount_minor"], 2700);
        assert_eq!(order["data"]["contact"]["email"], "guest@example.com");
        assert_eq!(
            order["data"]["billing_address"]["address_line1"],
            "1 Market Street"
        );
        assert_eq!(order["data"]["transitions"][0]["kind"], "created");
        assert_eq!(order["data"]["customer_id"], customer_id);

        let customer_orders = app
            .clone()
            .oneshot(with_customer_session(store_request(
                Method::GET,
                "/store/v1/customer/orders?limit=10",
                Some(full_secret),
                None,
                None,
            )))
            .await
            .unwrap();
        assert_eq!(customer_orders.status(), StatusCode::OK);
        let customer_orders = response_json(customer_orders).await;
        assert_eq!(customer_orders["data"][0]["id"], order_id);

        let admin_orders = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!(
                    "/admin/v1/stores/{}/orders?customer_id={customer_id}&email=guest%40example.com&status=pending", store_id.as_uuid()
                ),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(admin_orders.status(), StatusCode::OK);
        assert_eq!(response_json(admin_orders).await["data"][0]["id"], order_id);
        let order_replay = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &order_uri,
                    Some(full_secret),
                    Some("order"),
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(order_replay.status(), StatusCode::CREATED);
        assert_eq!(response_json(order_replay).await["data"]["id"], order_id);
        // GET /store/v1/orders/{order_id} is the ADR 0022 unauthenticated
        // single-Order lookup: it is gated on the API key's `orders:read`
        // scope alone (MachineActor-scoped, not shopper-possession-bound),
        // so a shopper token is irrelevant here and full_secret (which only
        // holds analytics:write/carts:write/checkout:write) is correctly
        // rejected at the scope check before any ownership question arises.
        let unrelated_order = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/orders/{order_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                unrelated_token,
            ))
            .await
            .unwrap();
        assert_eq!(unrelated_order.status(), StatusCode::FORBIDDEN);

        let orders_read_key = insert_key(&owner_pool, store_id, user_id, &["orders:read"]).await;
        let orders_read_secret = orders_read_key.plaintext.expose_secret();
        let machine_order_lookup = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                &format!("/store/v1/orders/{order_id}"),
                Some(orders_read_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(machine_order_lookup.status(), StatusCode::OK);
        assert_eq!(
            response_json(machine_order_lookup).await["data"]["id"],
            order_id
        );

        let missing_order_lookup = app
            .clone()
            .oneshot(store_request(
                Method::GET,
                &format!("/store/v1/orders/{}", Uuid::now_v7()),
                Some(orders_read_secret),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(missing_order_lookup.status(), StatusCode::NOT_FOUND);

        let mut snapshot_connection = runtime_pool.acquire().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, false)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *snapshot_connection)
            .await
            .unwrap();
        assert!(
            sqlx::query(
                "UPDATE sales.customer_shopper_links SET customer_id = customer_id \
                 WHERE customer_id = $1",
            )
            .bind(Uuid::parse_str(&customer_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.order_contacts SET email = 'tampered@example.com' \
                 WHERE order_id = $1",
            )
            .bind(Uuid::parse_str(order_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.order_addresses SET address_line1 = 'Tampered' \
                 WHERE order_id = $1",
            )
            .bind(Uuid::parse_str(order_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.checkout_shipping_selections SET amount_minor = 1 \
                 WHERE checkout_id = $1",
            )
            .bind(Uuid::parse_str(checkout_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.order_shipping_selections SET amount_minor = 1 \
                 WHERE order_id = $1",
            )
            .bind(Uuid::parse_str(order_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.checkout_tax_calculations SET rate_basis_points = 1 \
                 WHERE checkout_id = $1",
            )
            .bind(Uuid::parse_str(checkout_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.order_tax_calculations SET rate_basis_points = 1 \
                 WHERE order_id = $1",
            )
            .bind(Uuid::parse_str(order_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.checkout_promotion_calculations SET discount_amount_minor = 1 \
                 WHERE checkout_id = $1",
            )
            .bind(Uuid::parse_str(checkout_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "UPDATE sales.order_promotion_calculations SET discount_amount_minor = 1 \
                 WHERE order_id = $1",
            )
            .bind(Uuid::parse_str(order_id).unwrap())
            .execute(&mut *snapshot_connection)
            .await
            .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM sales.orders WHERE id = $1")
                .bind(Uuid::parse_str(order_id).unwrap())
                .execute(&mut *snapshot_connection)
                .await
                .is_err()
        );

        let terminal = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &checkout_uri,
                    Some(full_secret),
                    Some("checkout-again"),
                    Some(checkout_body),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(terminal.status(), StatusCode::CONFLICT);

        let fetched = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/checkouts/{checkout_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);
        assert_eq!(response_json(fetched).await["data"]["id"], checkout_id);

        // Uses orders_read_secret, not full_secret: GET /store/v1/orders/{order_id}
        // is the ADR 0022 Machine-scoped lookup (orders:read), not the
        // shopper-possession-bound path, so the shopper token here is inert.
        let fetched_order = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/orders/{order_id}"),
                    Some(orders_read_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(fetched_order.status(), StatusCode::OK);
        assert_eq!(response_json(fetched_order).await["data"]["id"], order_id);

        let disabled_provider_attempt = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &format!("/store/v1/orders/{order_id}/payment-attempts"),
                    Some(full_secret),
                    Some("disabled-provider-attempt"),
                    Some(json!({"provider": "stripe"})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(disabled_provider_attempt.status(), StatusCode::CONFLICT);

        let payment_attempt = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::POST,
                    &format!("/store/v1/orders/{order_id}/payment-attempts"),
                    Some(full_secret),
                    Some("payment-attempt"),
                    Some(json!({"provider": "testpay"})),
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(payment_attempt.status(), StatusCode::CREATED);
        let payment_attempt = response_json(payment_attempt).await;
        let payment_attempt_id = payment_attempt["data"]["id"].as_str().unwrap();
        assert_eq!(payment_attempt["data"]["status"], "pending");
        let payment_outbox = sqlx::query_as::<_, (Uuid, Uuid, String, Value, i32)>(
            "SELECT id, store_id, event_type, payload, attempts \
             FROM integration.outbox_events \
             WHERE aggregate_id = $1 AND event_type = 'payment.create_requested'",
        )
        .bind(Uuid::parse_str(payment_attempt_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        let payment_job = QueueJob {
            id: payment_outbox.0,
            store_id: payment_outbox.1,
            event_type: payment_outbox.2,
            payload: payment_outbox.3,
            attempts: payment_outbox.4 as u32,
        };
        let payment_command = payment_repository
            .prepare_provider_command(&payment_job)
            .await
            .unwrap();
        assert_eq!(payment_command.amount_minor, 2700);
        assert_eq!(
            payment_command.external_account_reference,
            provider_account_reference
        );
        payment_repository
            .record_provider_result(
                &payment_job,
                &ProviderCommandResult {
                    provider_reference: "pay_provider_1".into(),
                },
                test_clock.now(),
            )
            .await
            .unwrap();
        let client_action = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/payment-attempts/{payment_attempt_id}/client-action"),
                    Some(full_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(client_action.status(), StatusCode::OK);
        assert_eq!(client_action.headers()["cache-control"], "no-store");
        let client_action = response_json(client_action).await;
        assert_eq!(client_action["data"]["provider"], "testpay");
        assert_eq!(client_action["data"]["type"], "confirm_payment");
        assert_eq!(
            client_action["data"]["client_token"],
            "pay_provider_1_client_token"
        );
        let cross_store = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/payment-attempts/{payment_attempt_id}"),
                    Some(other_store_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(cross_store.status(), StatusCode::UNAUTHORIZED);

        let invalid_webhook = app
            .clone()
            .oneshot(
                Request::post("/webhooks/v1/payments/testpay")
                    .header("content-type", "application/json")
                    .header("x-payment-signature", "invalid")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_webhook.status(), StatusCode::UNAUTHORIZED);

        for (event_id, event_type) in [
            ("evt_authorized", "payment.authorized"),
            ("evt_captured", "payment.captured"),
        ] {
            let event = json!({
                "id": format!("{event_id}-{suffix}"),
                "event_type": event_type,
                "store": &provider_account_reference,
                "object": "pay_provider_1",
                "aggregate_id": payment_attempt_id,
            });
            let webhook = app
                .clone()
                .oneshot(webhook_request(event.clone()))
                .await
                .unwrap();
            assert_eq!(webhook.status(), StatusCode::ACCEPTED);
            assert_eq!(response_json(webhook).await["data"]["accepted"], true);
            let duplicate = app.clone().oneshot(webhook_request(event)).await.unwrap();
            assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
            assert_eq!(response_json(duplicate).await["data"]["accepted"], false);
            assert_eq!(
                payment_workers
                    .run_webhook_batch(
                        Uuid::now_v7(),
                        test_clock.now() + time::Duration::seconds(1),
                        10,
                    )
                    .await
                    .unwrap(),
                1
            );
        }

        let captured = app
            .clone()
            .oneshot(with_shopper_token(
                store_request(
                    Method::GET,
                    &format!("/store/v1/payment-attempts/{payment_attempt_id}"),
                    Some(full_secret),
                    None,
                    None,
                ),
                shopper_token,
            ))
            .await
            .unwrap();
        assert_eq!(captured.status(), StatusCode::OK);
        assert_eq!(response_json(captured).await["data"]["status"], "captured");
        let order_notification = sqlx::query_as::<_, (String, String, i32, Value)>(
            "SELECT recipient_email::text, template_key, template_version, template_payload \
             FROM notification.email_deliveries \
             WHERE store_id = $2 \
               AND semantic_event_type = 'order.confirmed' \
               AND template_payload ->> 'order_id' = $3",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(order_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(order_notification.0, "guest@example.com");
        assert_eq!(order_notification.1, "order_confirmation");
        assert_eq!(order_notification.2, 1);
        assert_eq!(order_notification.3["total_amount_minor"], 2700);

        let admin_order_uri = format!("/admin/v1/stores/{}/orders/{order_id}", store_id.as_uuid());
        let invalid_cancel = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{admin_order_uri}/cancel"),
                Some("cancel-confirmed"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_cancel.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let refund = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!(
                    "/admin/v1/stores/{}/payment-attempts/{payment_attempt_id}/refunds",
                    store_id.as_uuid()
                ),
                Some("refund"),
                Some(json!({"amount_minor": 1000})),
            ))
            .await
            .unwrap();
        assert_eq!(refund.status(), StatusCode::CREATED);
        let refund = response_json(refund).await;
        let refund_id = refund["data"]["id"].as_str().unwrap();
        let refund_outbox = sqlx::query_as::<_, (Uuid, Uuid, String, Value, i32)>(
            "SELECT id, store_id, event_type, payload, attempts \
             FROM integration.outbox_events \
             WHERE aggregate_id = $1 AND event_type = 'refund.create_requested'",
        )
        .bind(Uuid::parse_str(refund_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        let refund_job = QueueJob {
            id: refund_outbox.0,
            store_id: refund_outbox.1,
            event_type: refund_outbox.2,
            payload: refund_outbox.3,
            attempts: refund_outbox.4 as u32,
        };
        let refund_command = payment_repository
            .prepare_provider_command(&refund_job)
            .await
            .unwrap();
        assert_eq!(refund_command.amount_minor, 1000);
        assert_eq!(
            refund_command.payment_provider_reference.as_deref(),
            Some("pay_provider_1")
        );
        payment_repository
            .record_provider_result(
                &refund_job,
                &ProviderCommandResult {
                    provider_reference: "refund_provider_1".into(),
                },
                test_clock.now(),
            )
            .await
            .unwrap();
        let refund_webhook = app
            .clone()
            .oneshot(webhook_request(json!({
                "id": format!("evt_refund-{suffix}"),
                "event_type": "refund.succeeded",
                "store": &provider_account_reference,
                "object": "refund_provider_1",
                "aggregate_id": refund_id,
            })))
            .await
            .unwrap();
        assert_eq!(refund_webhook.status(), StatusCode::ACCEPTED);
        assert_eq!(
            payment_workers
                .run_webhook_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::seconds(1),
                    10,
                )
                .await
                .unwrap(),
            1
        );
        let refund_status: String =
            sqlx::query_scalar("SELECT status::text FROM payments.refunds WHERE id = $1")
                .bind(Uuid::parse_str(refund_id).unwrap())
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        assert_eq!(refund_status, "succeeded");

        let first_worker = Uuid::now_v7();
        let second_worker = Uuid::now_v7();
        let claimed_at = test_clock.now() + time::Duration::seconds(1);
        let (first_jobs, second_jobs) = tokio::join!(
            payment_repository.claim_outbox(
                first_worker,
                10,
                claimed_at,
                claimed_at - time::Duration::minutes(1),
            ),
            payment_repository.claim_outbox(
                second_worker,
                10,
                claimed_at,
                claimed_at - time::Duration::minutes(1),
            ),
        );
        let first_jobs = first_jobs.unwrap();
        let second_jobs = second_jobs.unwrap();
        assert_eq!(first_jobs.len() + second_jobs.len(), 2);
        let first_ids = first_jobs.iter().map(|job| job.id).collect::<Vec<_>>();
        assert!(second_jobs.iter().all(|job| !first_ids.contains(&job.id)));
        for job in first_jobs {
            payment_repository
                .finish_outbox(first_worker, job.id, Ok(()), claimed_at)
                .await
                .unwrap();
        }
        for job in second_jobs {
            payment_repository
                .finish_outbox(second_worker, job.id, Ok(()), claimed_at)
                .await
                .unwrap();
        }

        let recoverable_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO integration.outbox_events \
             (id, store_id, aggregate_type, aggregate_id, event_type, payload) \
             VALUES ($1, $2, $3, 'payment_attempt', $4, 'payment.create_requested', '{}'::jsonb)",
        )
        .bind(recoverable_id)
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(Uuid::now_v7())
        .execute(&owner_pool)
        .await
        .unwrap();
        let lease_started = test_clock.now() + time::Duration::seconds(1);
        let abandoned = payment_repository
            .claim_outbox(
                first_worker,
                1,
                lease_started,
                lease_started - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(abandoned[0].id, recoverable_id);
        assert_eq!(abandoned[0].attempts, 1);
        let not_stale = payment_repository
            .claim_outbox(
                second_worker,
                1,
                lease_started,
                lease_started - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert!(not_stale.is_empty());
        let recovered_at = lease_started + time::Duration::seconds(61);
        let recovered = payment_repository
            .claim_outbox(
                second_worker,
                1,
                recovered_at,
                recovered_at - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(recovered[0].id, recoverable_id);
        assert_eq!(recovered[0].attempts, 2);
        payment_repository
            .finish_outbox(second_worker, recoverable_id, Ok(()), recovered_at)
            .await
            .unwrap();
        assert!(
            payment_repository
                .finish_outbox(first_worker, recoverable_id, Ok(()), recovered_at)
                .await
                .is_err()
        );

        let recoverable_webhook_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO integration.webhook_inbox \
             (id, store_id, provider, provider_event_id, event_type, \
              external_account_reference, payload, available_at, verified_at) \
             VALUES ($1, $2, $3, 'testpay', $4, 'payment.authorized', $5, '{}'::jsonb, $6, $6)",
        )
        .bind(recoverable_webhook_id)
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(format!("recoverable-{suffix}"))
        .bind(&provider_account_reference)
        .bind(lease_started)
        .execute(&owner_pool)
        .await
        .unwrap();
        let abandoned_webhook = payment_repository
            .claim_webhooks(
                first_worker,
                1,
                lease_started,
                lease_started - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(abandoned_webhook[0].id, recoverable_webhook_id);
        let recovered_webhook = payment_repository
            .claim_webhooks(
                second_worker,
                1,
                recovered_at,
                recovered_at - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(recovered_webhook[0].id, recoverable_webhook_id);
        assert_eq!(recovered_webhook[0].attempts, 2);
        payment_repository
            .finish_webhook(second_worker, recoverable_webhook_id, Ok(()), recovered_at)
            .await
            .unwrap();

        let dead_letter_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO integration.outbox_events \
             (id, store_id, aggregate_type, aggregate_id, event_type, payload) \
             VALUES ($1, $2, $3, 'payment_attempt', $4, 'payment.create_requested', '{}'::jsonb)",
        )
        .bind(dead_letter_id)
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(Uuid::now_v7())
        .execute(&owner_pool)
        .await
        .unwrap();
        for attempt in 1..=8 {
            let claim_time = test_clock.now();
            sqlx::query("UPDATE integration.outbox_events SET available_at = $2 WHERE id = $1")
                .bind(dead_letter_id)
                .bind(claim_time)
                .execute(&owner_pool)
                .await
                .unwrap();
            let worker_id = Uuid::now_v7();
            let jobs = payment_repository
                .claim_outbox(
                    worker_id,
                    1,
                    claim_time,
                    claim_time - time::Duration::minutes(1),
                )
                .await
                .unwrap();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].attempts, attempt);
            payment_repository
                .finish_outbox(
                    worker_id,
                    dead_letter_id,
                    Err("provider unavailable".into()),
                    test_clock.now(),
                )
                .await
                .unwrap();
        }
        let dead_letter: (String, i32) = sqlx::query_as(
            "SELECT status::text, attempts FROM integration.outbox_events WHERE id = $1",
        )
        .bind(dead_letter_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(dead_letter, ("dead_letter".into(), 8));

        let fulfillment_base = format!(
            "/admin/v1/stores/{}/orders/{order_id}/fulfillments",
            store_id.as_uuid()
        );
        let fulfillment = app
            .clone()
            .oneshot(request(
                Method::POST,
                &fulfillment_base,
                Some("fulfillment-create"),
                Some(json!({
                    "allocations": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "quantity": 1
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(fulfillment.status(), StatusCode::CREATED);
        let fulfillment = response_json(fulfillment).await;
        let fulfillment_id = fulfillment["data"]["id"].as_str().unwrap();
        assert_eq!(fulfillment["data"]["status"], "pending");

        let excessive_fulfillment = app
            .clone()
            .oneshot(request(
                Method::POST,
                &fulfillment_base,
                Some("fulfillment-excessive"),
                Some(json!({
                    "allocations": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "quantity": 2
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(excessive_fulfillment.status(), StatusCode::CONFLICT);

        let fulfillment_uri = format!(
            "/admin/v1/stores/{}/fulfillments/{fulfillment_id}",
            store_id.as_uuid()
        );
        let shipped = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{fulfillment_uri}/ship"),
                Some("fulfillment-ship"),
                Some(json!({
                    "carrier": "test",
                    "tracking_number": format!("track-{suffix}")
                })),
            ))
            .await
            .unwrap();
        assert_eq!(shipped.status(), StatusCode::OK);
        assert_eq!(response_json(shipped).await["data"]["status"], "shipped");
        let delivered = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{fulfillment_uri}/deliver"),
                Some("fulfillment-deliver"),
                Some(json!({})),
            ))
            .await
            .unwrap();
        assert_eq!(delivered.status(), StatusCode::OK);
        assert_eq!(
            response_json(delivered).await["data"]["status"],
            "delivered"
        );
        assert!(
            fulfillment_workers
                .run_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::seconds(1),
                    10,
                )
                .await
                .unwrap()
                >= 2
        );
        let reconciled_order = app
            .clone()
            .oneshot(request(Method::GET, &admin_order_uri, None, None))
            .await
            .unwrap();
        assert_eq!(reconciled_order.status(), StatusCode::OK);
        let reconciled_order = response_json(reconciled_order).await;
        assert_eq!(
            reconciled_order["data"]["fulfillment_status"],
            "partially_fulfilled"
        );
        assert_eq!(
            reconciled_order["data"]["delivery_status"],
            "partially_delivered"
        );
        let shipped_event_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM integration.outbox_events WHERE aggregate_id = $1 \
             AND event_type = 'fulfillment.shipped'",
        )
        .bind(Uuid::parse_str(fulfillment_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE integration.outbox_events SET status = 'processing', processed_at = NULL, \
                    locked_by = $2, locked_at = $3 WHERE id = $1",
        )
        .bind(shipped_event_id)
        .bind(Uuid::now_v7())
        .bind(test_clock.now() - time::Duration::minutes(2))
        .execute(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            fulfillment_workers
                .run_batch(Uuid::now_v7(), test_clock.now(), 10)
                .await
                .unwrap(),
            1
        );
        let transition_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sales.order_fulfillment_transitions \
             WHERE source_event_id = $1",
        )
        .bind(shipped_event_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(transition_count, 1);

        let provider_uri = format!(
            "/admin/v1/stores/{}/shipping-provider-accounts",
            store_id.as_uuid()
        );
        let shipping_provider_account = app
            .clone()
            .oneshot(request(
                Method::POST,
                &provider_uri,
                Some("shipping-provider-create"),
                Some(json!({
                    "provider": "easypost",
                    "display_name": "EasyPost Test",
                    "credential_secret_reference": "env://CHAOS_SHIPPING_SECRET_EASYPOST",
                    "origin": {
                        "name": "Test Warehouse",
                        "address_line_1": "10 Warehouse Road",
                        "city": "Singapore",
                        "postal_code": "018956",
                        "country_code": "SG",
                        "phone": "+6591234567",
                        "email": "warehouse@example.com"
                    },
                    "enabled": true
                })),
            ))
            .await
            .unwrap();
        assert_eq!(shipping_provider_account.status(), StatusCode::CREATED);
        let shipping_provider_account = response_json(shipping_provider_account).await;
        let shipping_provider_account_id =
            shipping_provider_account["data"]["id"].as_str().unwrap();

        let provider_fulfillment = app
            .clone()
            .oneshot(request(
                Method::POST,
                &fulfillment_base,
                Some("provider-fulfillment-create"),
                Some(json!({
                    "allocations": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "quantity": 1
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(provider_fulfillment.status(), StatusCode::CREATED);
        let provider_fulfillment = response_json(provider_fulfillment).await;
        let provider_fulfillment_id = provider_fulfillment["data"]["id"].as_str().unwrap();
        let provider_fulfillment_uri = format!(
            "/admin/v1/stores/{}/fulfillments/{provider_fulfillment_id}",
            store_id.as_uuid()
        );
        let quote_body = json!({
            "provider_account_id": shipping_provider_account_id,
            "length_millimetres": 254,
            "width_millimetres": 127,
            "height_millimetres": 51,
            "weight_grams": 454
        });
        let cross_store_quote = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!(
                    "/admin/v1/stores/{}/fulfillments/{provider_fulfillment_id}/shipping-rates",
                    other_store_id.as_uuid()
                ),
                Some("cross-store-provider-rate-quote"),
                Some(quote_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(cross_store_quote.status(), StatusCode::CONFLICT);
        let quote = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{provider_fulfillment_uri}/shipping-rates"),
                Some("provider-rate-quote"),
                Some(quote_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(quote.status(), StatusCode::CREATED);
        let quote = response_json(quote).await;
        assert_eq!(quote["data"][0]["amount_minor"], 1234);
        let rate_quote_id = quote["data"][0]["id"].as_str().unwrap();
        let quote_replay = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{provider_fulfillment_uri}/shipping-rates"),
                Some("provider-rate-quote"),
                Some(quote_body),
            ))
            .await
            .unwrap();
        assert_eq!(quote_replay.status(), StatusCode::CREATED);
        assert_eq!(
            response_json(quote_replay).await["data"][0]["id"],
            rate_quote_id
        );

        let label_body = json!({"rate_quote_id": rate_quote_id});
        let label = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{provider_fulfillment_uri}/shipping-label"),
                Some("provider-label-purchase"),
                Some(label_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(label.status(), StatusCode::CREATED);
        let label = response_json(label).await;
        assert_eq!(label["data"]["carrier"], "USPS");
        assert_eq!(label["data"]["label_media_type"], "image/png");
        let shipping_label_id = label["data"]["id"].as_str().unwrap();
        let label_replay = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{provider_fulfillment_uri}/shipping-label"),
                Some("provider-label-purchase"),
                Some(label_body),
            ))
            .await
            .unwrap();
        assert_eq!(label_replay.status(), StatusCode::CREATED);
        assert_eq!(
            response_json(label_replay).await["data"]["id"],
            shipping_label_id
        );
        assert_eq!(shipping_purchase_count.load(Ordering::SeqCst), 0);

        let abandoned_tracking_worker = Uuid::now_v7();
        let abandoned_tracking = shipping_repository
            .claim_tracking(
                abandoned_tracking_worker,
                10,
                test_clock.now(),
                test_clock.now() - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(abandoned_tracking.len(), 1);
        let recovered_tracking_at = test_clock.now() + time::Duration::minutes(2);
        assert_eq!(
            fulfillment_workers
                .run_tracking_batch(Uuid::now_v7(), recovered_tracking_at, 10)
                .await
                .unwrap(),
            1
        );
        assert!(
            shipping_repository
                .finish_tracking(
                    abandoned_tracking_worker,
                    &abandoned_tracking[0],
                    Ok(ShippingTrackingSnapshot {
                        status: ProviderTrackingStatus::Delivered,
                        status_detail: None,
                        estimated_delivery_at: None,
                        observed_at: recovered_tracking_at,
                    }),
                    recovered_tracking_at,
                )
                .await
                .is_err()
        );
        let provider_fulfillment_status: String =
            sqlx::query_scalar("SELECT status::text FROM fulfillment.fulfillments WHERE id = $1")
                .bind(Uuid::parse_str(provider_fulfillment_id).unwrap())
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        assert_eq!(provider_fulfillment_status, "delivered");
        let tracking_status: String = sqlx::query_scalar(
            "SELECT tracking_status FROM fulfillment.shipping_labels WHERE id = $1",
        )
        .bind(Uuid::parse_str(shipping_label_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(tracking_status, "delivered");
        assert!(
            fulfillment_workers
                .run_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::seconds(1),
                    10,
                )
                .await
                .unwrap()
                >= 2
        );

        let cancellation = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{provider_fulfillment_uri}/shipping-label/cancel"),
                Some("provider-label-cancel"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(cancellation.status(), StatusCode::OK);
        assert_eq!(
            response_json(cancellation).await["data"]["cancellation_status"],
            "submitted"
        );
        let abandoned_cancellation_worker = Uuid::now_v7();
        let cancellation_reconcile_at = test_clock.now() + time::Duration::minutes(15);
        let abandoned_cancellation = shipping_repository
            .claim_cancellations(
                abandoned_cancellation_worker,
                10,
                cancellation_reconcile_at,
                cancellation_reconcile_at - time::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(abandoned_cancellation.len(), 1);
        let recovered_cancellation_at = cancellation_reconcile_at + time::Duration::minutes(2);
        assert_eq!(
            fulfillment_workers
                .run_cancellation_batch(Uuid::now_v7(), recovered_cancellation_at, 10)
                .await
                .unwrap(),
            1
        );
        assert!(
            shipping_repository
                .finish_cancellation(
                    abandoned_cancellation_worker,
                    &abandoned_cancellation[0],
                    Ok(ShippingCancellationStatus::Cancelled),
                    recovered_cancellation_at,
                )
                .await
                .is_err()
        );
        let cancellation_status: String = sqlx::query_scalar(
            "SELECT cancellation_status FROM fulfillment.shipping_labels WHERE id = $1",
        )
        .bind(Uuid::parse_str(shipping_label_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(cancellation_status, "cancelled");

        let return_base = format!(
            "/admin/v1/stores/{}/orders/{order_id}/returns",
            store_id.as_uuid()
        );
        let returned = app
            .clone()
            .oneshot(request(
                Method::POST,
                &return_base,
                Some("return-create"),
                Some(json!({
                    "lines": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "quantity": 1
                    }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(returned.status(), StatusCode::CREATED);
        let returned = response_json(returned).await;
        let return_id = returned["data"]["id"].as_str().unwrap();
        let return_uri = format!(
            "/admin/v1/stores/{}/returns/{return_id}",
            store_id.as_uuid()
        );
        for (operation, key, body, expected) in [
            ("authorize", "return-authorize", json!({}), "authorized"),
            (
                "receive",
                "return-receive",
                json!({
                    "receipt": [{
                        "product_variant_id": variant_id.as_uuid(),
                        "disposition": "restock",
                        "inventory_location_id": location_id.as_uuid()
                    }]
                }),
                "received",
            ),
            ("complete", "return-complete", json!({}), "completed"),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    Method::POST,
                    &format!("{return_uri}/{operation}"),
                    Some(key),
                    Some(body),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_json(response).await["data"]["status"], expected);
        }

        assert_eq!(
            fulfillment_workers
                .run_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::seconds(1),
                    10,
                )
                .await
                .unwrap(),
            1
        );
        let return_detail = app
            .clone()
            .oneshot(request(Method::GET, &return_uri, None, None))
            .await
            .unwrap();
        assert_eq!(return_detail.status(), StatusCode::OK);
        let return_detail = response_json(return_detail).await;
        assert_eq!(return_detail["data"]["refund_amount_minor"], 1100);
        assert_eq!(return_detail["data"]["currency"], "USD");
        let coordinated_refund_id = return_detail["data"]["refund_id"]
            .as_str()
            .expect("completed Return must expose its coordinated Refund");
        let return_event_status: String = sqlx::query_scalar(
            "SELECT status::text FROM integration.outbox_events WHERE aggregate_type = 'return' \
             AND aggregate_id = $1 AND event_type = 'return.completed'",
        )
        .bind(Uuid::parse_str(return_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(return_event_status, "processed");
        let return_backlog: (Option<String>, i64, i64) = sqlx::query_as(
            "SELECT consumer_owner, pending, processed \
             FROM integration.event_consumer_backlog() WHERE event_type = 'return.completed'",
        )
        .fetch_one(&runtime_pool)
        .await
        .unwrap();
        assert_eq!(return_backlog.0.as_deref(), Some("fulfillment.operations"));
        assert_eq!(return_backlog.1, 0);
        assert!(return_backlog.2 >= 1);
        let coordinated_refund_outbox: (String, i64) = sqlx::query_as(
            "SELECT status::text, (payload->>'amount_minor')::bigint \
             FROM integration.outbox_events WHERE aggregate_id = $1 \
               AND event_type = 'refund.create_requested'",
        )
        .bind(Uuid::parse_str(coordinated_refund_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(coordinated_refund_outbox, ("pending".into(), 1100));
        let return_event_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM integration.outbox_events WHERE aggregate_type = 'return' \
             AND aggregate_id = $1 AND event_type = 'return.completed'",
        )
        .bind(Uuid::parse_str(return_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE integration.outbox_events SET status = 'processing', processed_at = NULL, \
                    locked_by = $2, locked_at = $3 WHERE id = $1",
        )
        .bind(return_event_id)
        .bind(Uuid::now_v7())
        .bind(test_clock.now() - time::Duration::minutes(2))
        .execute(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            fulfillment_workers
                .run_batch(Uuid::now_v7(), test_clock.now(), 10)
                .await
                .unwrap(),
            1
        );
        let linked_refund_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM payments.refunds AS refund \
             INNER JOIN fulfillment.returns AS return_record ON return_record.refund_id = refund.id \
             WHERE return_record.id = $1",
        )
        .bind(Uuid::parse_str(return_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(linked_refund_count, 1);
        let pending_commerce_facts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM integration.outbox_events AS event \
             INNER JOIN integration.event_consumer_registry AS registry \
               ON registry.event_type = event.event_type \
             WHERE registry.consumer_owner = 'analytics.commerce_fact_ingestor' \
               AND event.status = 'pending'",
        )
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(pending_commerce_facts, 6);
        assert_eq!(
            analytics_workers
                .run_commerce_fact_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::seconds(2),
                    10,
                )
                .await
                .unwrap(),
            6
        );
        let commerce_facts: Vec<(String, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT fact_name::text, amount_minor, currency::text \
             FROM analytics.commerce_facts WHERE store_id = $2 \
             ORDER BY fact_name",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .fetch_all(&owner_pool)
        .await
        .unwrap();
        assert_eq!(commerce_facts.len(), 6);
        for fact_name in [
            "order_created",
            "payment_captured",
            "refund_succeeded",
            "fulfillment_shipped",
            "return_completed",
        ] {
            assert!(commerce_facts.iter().any(|fact| fact.0 == fact_name));
        }
        assert!(
            commerce_facts
                .iter()
                .filter(|fact| fact.1.is_some())
                .all(|fact| fact.1.unwrap() >= 0 && fact.2.as_deref() == Some("USD"))
        );
        assert_eq!(
            analytics_workers
                .run_attribution_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::minutes(4),
                    10,
                )
                .await
                .unwrap(),
            0
        );
        let abandoned_attribution_worker = Uuid::now_v7();
        let abandoned_attribution_jobs = analytics_repository
            .claim_attribution(
                abandoned_attribution_worker,
                10,
                test_clock.now() + time::Duration::minutes(6),
                test_clock.now() + time::Duration::minutes(4),
            )
            .await
            .unwrap();
        assert_eq!(abandoned_attribution_jobs.len(), 1);
        assert_eq!(
            analytics_workers
                .run_attribution_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::minutes(7),
                    10,
                )
                .await
                .unwrap(),
            1
        );
        let stale_attribution_result = analytics_repository
            .finish_attribution(
                abandoned_attribution_worker,
                &abandoned_attribution_jobs[0],
                Ok(()),
                test_clock.now() + time::Duration::minutes(7),
            )
            .await;
        assert!(matches!(
            stale_attribution_result,
            Err(ApplicationError::Conflict { .. })
        ));
        type AttributionResultRow = (
            String,
            bool,
            Option<String>,
            bool,
            i16,
            Option<String>,
            Option<String>,
        );
        let attribution_results: Vec<AttributionResultRow> = sqlx::query_as(
            "SELECT attribution_model::text, is_direct, campaign_source, \
                        advertising_export_eligible, model_version, consent_policy_version, \
                        collection_policy_version \
                   FROM analytics.attribution_results \
                  WHERE store_id = $2 AND order_id = $3 \
                  ORDER BY attribution_model",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(Uuid::parse_str(order_id).unwrap())
        .fetch_all(&owner_pool)
        .await
        .unwrap();
        assert_eq!(attribution_results.len(), 2);
        assert_eq!(attribution_results[0].0, "first_touch");
        assert!(!attribution_results[0].1);
        assert_eq!(attribution_results[0].2.as_deref(), Some("first-source"));
        assert!(attribution_results[0].3);
        assert_eq!(attribution_results[0].4, 1);
        assert_eq!(
            attribution_results[0].5.as_deref(),
            Some("cmp-attribution-v1")
        );
        assert_eq!(attribution_results[0].6.as_deref(), Some("store-v4"));
        assert_eq!(attribution_results[1].0, "last_touch");
        assert_eq!(attribution_results[1].2.as_deref(), Some("last-source"));
        assert!(attribution_results[1].3);
        let pending_exports: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM analytics.export_deliveries \
             WHERE store_id = $2 AND delivery_status = 'pending'",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(pending_exports, 12);
        assert_eq!(
            analytics_workers
                .run_export_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::minutes(7),
                    20,
                )
                .await
                .unwrap(),
            12
        );
        assert_eq!(analytics_delivery_count.load(Ordering::SeqCst), 12);
        assert_eq!(
            analytics_workers
                .run_export_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::minutes(7),
                    20,
                )
                .await
                .unwrap(),
            0
        );

        let report_date = test_clock.now().date().to_string();
        let daily_reports = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!(
                    "/admin/v1/stores/{}/analytics-reports/daily?from={report_date}&to={report_date}", store_id.as_uuid()
                ),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(daily_reports.status(), StatusCode::OK);
        let daily_reports = response_json(daily_reports).await;
        assert_eq!(daily_reports["data"]["commerce"][0]["orders_created"], 1);
        assert_eq!(daily_reports["data"]["commerce"][0]["payments_captured"], 1);
        assert_eq!(
            daily_reports["data"]["attribution"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(daily_reports["data"]["attribution"][0]["model_version"], 1);
        let unbounded_daily_reports = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!(
                    "/admin/v1/stores/{}/analytics-reports/daily?from=2025-01-01&to=2025-04-03",
                    store_id.as_uuid()
                ),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            unbounded_daily_reports.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let cross_account_daily_reports = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!(
                    "/admin/v1/stores/{}/analytics-reports/daily?from={report_date}&to={report_date}", isolated_store_id.as_uuid()
                ),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(cross_account_daily_reports.status(), StatusCode::FORBIDDEN);
        let mut reporting_rls = reporting_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *reporting_rls)
            .await
            .unwrap();
        let visible_daily_commerce: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.daily_commerce_reports")
                .fetch_one(&mut *reporting_rls)
                .await
                .unwrap();
        assert!(visible_daily_commerce > 0);
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(isolated_store_id.as_uuid().to_string())
            .execute(&mut *reporting_rls)
            .await
            .unwrap();
        let isolated_daily_commerce: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.daily_commerce_reports")
                .fetch_one(&mut *reporting_rls)
                .await
                .unwrap();
        assert_eq!(isolated_daily_commerce, 0);
        reporting_rls.rollback().await.unwrap();
        assert!(
            sqlx::query("UPDATE analytics.daily_commerce_reports SET orders_created = 0")
                .execute(&reporting_pool)
                .await
                .is_err()
        );

        let mut attribution_rebuild = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *attribution_rebuild)
            .await
            .unwrap();
        let rebuilt: i64 =
            sqlx::query_scalar("SELECT analytics.rebuild_store_attribution($1,$2,1::smallint,$3)")
                .bind(store_id.as_uuid())
                .bind(store_id.as_uuid())
                .bind(test_clock.now() + time::Duration::minutes(8))
                .fetch_one(&mut *attribution_rebuild)
                .await
                .unwrap();
        assert_eq!(rebuilt, 1);
        attribution_rebuild.commit().await.unwrap();
        assert_eq!(
            analytics_workers
                .run_attribution_batch(
                    Uuid::now_v7(),
                    test_clock.now() + time::Duration::minutes(8),
                    10,
                )
                .await
                .unwrap(),
            1
        );
        let rebuilt_result_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM analytics.attribution_results \
             WHERE store_id = $2 AND order_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(Uuid::parse_str(order_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(rebuilt_result_count, 2);
        let mut attribution_rls = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *attribution_rls)
            .await
            .unwrap();
        let visible_attribution_results: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.attribution_results")
                .fetch_one(&mut *attribution_rls)
                .await
                .unwrap();
        assert_eq!(visible_attribution_results, 2);
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(isolated_store_id.as_uuid().to_string())
            .execute(&mut *attribution_rls)
            .await
            .unwrap();
        let isolated_attribution_results: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.attribution_results")
                .fetch_one(&mut *attribution_rls)
                .await
                .unwrap();
        assert_eq!(isolated_attribution_results, 0);
        attribution_rls.rollback().await.unwrap();
        let attribution_erasure = app
            .clone()
            .oneshot(request(
                Method::POST,
                &erasure_uri,
                Some("erase-attribution-subject"),
                Some(json!({"anonymous_id": attribution_anonymous_id})),
            ))
            .await
            .unwrap();
        assert_eq!(attribution_erasure.status(), StatusCode::ACCEPTED);
        let attribution_erasure_id = response_json(attribution_erasure).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            analytics_workers
                .run_erasure_batch(test_clock.now() + time::Duration::minutes(9), 10)
                .await
                .unwrap()
                .requests_completed,
            1
        );
        let completed_attribution_erasure = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{erasure_uri}/{attribution_erasure_id}"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(completed_attribution_erasure.status(), StatusCode::OK);
        let completed_attribution_erasure = response_json(completed_attribution_erasure).await;
        assert_eq!(
            completed_attribution_erasure["data"]["behavior_events_deleted"],
            3
        );
        assert_eq!(
            completed_attribution_erasure["data"]["attribution_results_deleted"],
            2
        );
        let erased_attribution_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM analytics.attribution_results \
             WHERE store_id = $2 AND order_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(Uuid::parse_str(order_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(erased_attribution_count, 0);
        let replay_fact_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM analytics.commerce_facts WHERE  \
             AND store_id = $2 AND fact_name = 'payment_captured'",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE integration.outbox_events SET status = 'processing', processed_at = NULL, \
                    locked_by = $2, locked_at = $3 WHERE id = $1",
        )
        .bind(replay_fact_id)
        .bind(Uuid::now_v7())
        .bind(test_clock.now() - time::Duration::minutes(2))
        .execute(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            analytics_workers
                .run_commerce_fact_batch(Uuid::now_v7(), test_clock.now(), 10)
                .await
                .unwrap(),
            1
        );
        let commerce_fact_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.commerce_facts")
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        assert_eq!(commerce_fact_count, 6);
        let mut commerce_fact_rls = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *commerce_fact_rls)
            .await
            .unwrap();
        let visible_commerce_facts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.commerce_facts")
                .fetch_one(&mut *commerce_fact_rls)
                .await
                .unwrap();
        assert_eq!(visible_commerce_facts, 6);
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(isolated_store_id.as_uuid().to_string())
            .execute(&mut *commerce_fact_rls)
            .await
            .unwrap();
        let cross_account_commerce_facts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM analytics.commerce_facts")
                .fetch_one(&mut *commerce_fact_rls)
                .await
                .unwrap();
        assert_eq!(cross_account_commerce_facts, 0);
        commerce_fact_rls.rollback().await.unwrap();
        assert!(
            sqlx::query("UPDATE analytics.commerce_facts SET amount_minor = 1 WHERE id = $1")
                .bind(replay_fact_id)
                .execute(&runtime_pool)
                .await
                .is_err()
        );
        let registered_owners: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT event_type, consumer_owner FROM integration.event_consumer_registry \
             ORDER BY event_type",
        )
        .fetch_all(&runtime_pool)
        .await
        .unwrap();
        assert_eq!(registered_owners.len(), 12);
        assert!(registered_owners.contains(&(
            "payment.create_requested".into(),
            Some("payments.provider_dispatch".into())
        )));
        assert!(registered_owners.contains(&(
            "search.product.changed".into(),
            Some("search.product_indexer".into())
        )));
        for event_type in [
            "fulfillment.shipped",
            "fulfillment.delivered",
            "fulfillment.cancelled",
            "return.completed",
        ] {
            assert!(
                registered_owners
                    .contains(&(event_type.into(), Some("fulfillment.operations".into())))
            );
        }
        for event_type in [
            "analytics.order.created",
            "analytics.payment.captured",
            "analytics.refund.succeeded",
            "analytics.fulfillment.shipped",
            "analytics.return.completed",
        ] {
            assert!(registered_owners.contains(&(
                event_type.into(),
                Some("analytics.commerce_fact_ingestor".into())
            )));
        }
        assert!(
            sqlx::query(
                "UPDATE integration.event_consumer_registry \
                 SET consumer_owner = 'unauthorized.owner' \
                 WHERE event_type = 'return.completed'",
            )
            .execute(&runtime_pool)
            .await
            .is_err()
        );
        let restocks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM inventory.stock_ledger_entries WHERE kind = 'return_restock' \
             AND store_id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(restocks, 1);

        let stock: (i64, i64) = sqlx::query_as(
            "SELECT on_hand_quantity, reserved_quantity FROM inventory.stock_items \
             WHERE store_id = $2 AND product_variant_id = $3",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(variant_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(stock, (1, 0));
        let reservation_status: String = sqlx::query_scalar(
            "SELECT status::text FROM inventory.inventory_reservations \
             WHERE store_id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(reservation_status, "consumed");
    }
}
