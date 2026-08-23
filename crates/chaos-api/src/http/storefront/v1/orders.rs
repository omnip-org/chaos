use axum::{
    Router,
    extract::State,
    routing::{get, post},
};
use chaos_application::ports::{OrderDetail, OrderLineItem};
use chaos_domain::sales::OrderId;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiDateTime, ApiJson, ApiPath, ApiResponse, ApiState, CartShopper, OrderLookupMachine,
};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/orders/{order_id}", get(get_order))
        .route("/order-tracking-orders", post(get_tracked_order))
}

#[derive(Deserialize)]
struct TrackingTokenBody {
    tracking_token: String,
}

#[derive(Deserialize)]
struct OrderPath {
    order_id: Uuid,
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
struct OrderData {
    id: Uuid,
    order_number: String,
    price_list_id: Uuid,
    currency: String,
    status: &'static str,
    payment_status: &'static str,
    shipping_status: &'static str,
    contact: OrderContactData,
    #[serde(skip_serializing_if = "Option::is_none")]
    billing_address: Option<PostalAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_address: Option<PostalAddressData>,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    refunded_amount_minor: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    stripe_checkout_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stripe_payment_intent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stripe_charge_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_provider_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_tracking_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_tracking_url: Option<String>,
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

async fn get_order(
    State(state): State<ApiState>,
    CartShopper(actor): CartShopper,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<ApiResponse<OrderData>, crate::http::ApiError> {
    let order = state
        .storefront_sales
        .get_order(&actor, OrderId::from_uuid(path.order_id))
        .await?;
    Ok(ApiResponse::ok(order_data(order)?))
}

async fn get_tracked_order(
    State(state): State<ApiState>,
    OrderLookupMachine(actor): OrderLookupMachine,
    ApiJson(body): ApiJson<TrackingTokenBody>,
) -> Result<ApiResponse<OrderData>, crate::http::ApiError> {
    let order = state
        .storefront_sales
        .get_tracked_order(
            &actor,
            &SecretString::from(body.tracking_token),
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

fn order_data(order: OrderDetail) -> Result<OrderData, chaos_application::ApplicationError> {
    Ok(OrderData {
        id: order.id.as_uuid(),
        order_number: order.order_number.as_str().into(),
        price_list_id: order.price_list_id.as_uuid(),
        currency: order.currency.as_str().to_owned(),
        status: order.status.as_str(),
        payment_status: order.payment_status.as_str(),
        shipping_status: order.shipping_status.as_str(),
        contact: contact_data(order.identity.contact()),
        billing_address: order.identity.billing_address().map(address_data),
        shipping_address: order.identity.shipping_address().map(address_data),
        subtotal_amount_minor: order.subtotal_amount_minor,
        discount_amount_minor: order.discount_amount_minor,
        tax_amount_minor: order.tax_amount_minor,
        shipping_amount_minor: order.shipping_amount_minor,
        total_amount_minor: order.total_amount_minor,
        refunded_amount_minor: order.refunded_amount_minor,
        stripe_checkout_session_id: order.stripe_checkout_session_id,
        stripe_payment_intent_id: order.stripe_payment_intent_id,
        stripe_charge_id: order.stripe_charge_id,
        shipping_provider: order.shipping_provider,
        shipping_provider_reference: order.shipping_provider_reference,
        shipping_tracking_number: order.shipping_tracking_number,
        shipping_tracking_url: order.shipping_tracking_url,
        lines: order.lines.into_iter().map(order_line_data).collect(),
        transitions: order
            .transitions
            .into_iter()
            .map(|transition| OrderTransitionData {
                id: transition.id,
                from_status: transition.from_status.map(|status| status.as_str()),
                to_status: transition.to_status.as_str(),
                kind: transition.kind,
                occurred_at: transition.occurred_at.into(),
            })
            .collect(),
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
