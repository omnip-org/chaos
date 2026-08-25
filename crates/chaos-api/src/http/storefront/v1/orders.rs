use axum::{
    Router,
    extract::State,
    routing::{get, post},
};
use chaos_core::contracts::{
    OrderDetail, OrderFulfillmentItem, OrderLineItem, OrderPaymentAttemptItem, OrderRefundItem,
};
use chaos_domain::sales::OrderId;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiDateTime, ApiJson, ApiPath, ApiResponse, ApiState, CartShopper, OrderLookupMachine,
};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/orders/{order_id}", get(get_order))
        .route("/orders/tracking", post(get_tracked_order))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
}

#[derive(Serialize)]
struct PaymentAttemptData {
    status: &'static str,
    amount_minor: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_reference_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct RefundData {
    id: Uuid,
    status: &'static str,
    amount_minor: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_reference_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct FulfillmentData {
    id: Uuid,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracking_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracking_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipped_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivered_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cancelled_at: Option<ApiDateTime>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

/// The subset of a Fulfillment safe to expose on the order-tracking view:
/// shipping progress and carrier tracking, without the internal Store
/// provider-account id.
#[derive(Serialize)]
struct TrackedFulfillmentData {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracking_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracking_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipped_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivered_at: Option<ApiDateTime>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    payment_provider: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payment_provider_reference_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_provider: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_provider_reference_id: Option<String>,
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
    lines: Vec<OrderLineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payment_attempt: Option<PaymentAttemptData>,
    refunds: Vec<RefundData>,
    fulfillments: Vec<FulfillmentData>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

/// The order-tracking view served through the long-lived capability link.
/// Contact details and the full billing/shipping address are deliberately
/// left out — the shopper already has that in the confirmation email, and
/// this URL is designed to be shareable without leaking it further.
#[derive(Serialize)]
struct TrackedOrderData {
    id: Uuid,
    order_number: String,
    currency: String,
    status: &'static str,
    payment_status: &'static str,
    shipping_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_country_code: Option<String>,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    refunded_amount_minor: i64,
    fulfillments: Vec<TrackedFulfillmentData>,
    lines: Vec<OrderLineData>,
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
) -> Result<ApiResponse<TrackedOrderData>, crate::http::ApiError> {
    let order = state
        .storefront_sales
        .get_tracked_order(
            &actor,
            &SecretString::from(body.tracking_token),
            state.clock.now(),
        )
        .await?;
    Ok(ApiResponse::ok(tracked_order_data(order)))
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

fn order_data(order: OrderDetail) -> Result<OrderData, chaos_core::ApplicationError> {
    Ok(OrderData {
        id: order.id.as_uuid(),
        order_number: order.order_number.as_str().into(),
        price_list_id: order.price_list_id.as_uuid(),
        currency: order.currency.as_str().to_owned(),
        status: order.status.as_str(),
        payment_status: order.payment_status.as_str(),
        shipping_status: order.shipping_status.as_str(),
        payment_provider: order.payment_provider.map(|value| value.as_str()),
        payment_provider_reference_id: order.payment_provider_reference_id,
        shipping_provider: order.shipping_provider.map(|value| value.as_str()),
        shipping_provider_reference_id: order.shipping_provider_reference_id,
        contact: contact_data(order.identity.contact()),
        billing_address: order.identity.billing_address().map(address_data),
        shipping_address: order.identity.shipping_address().map(address_data),
        subtotal_amount_minor: order.subtotal_amount_minor,
        discount_amount_minor: order.discount_amount_minor,
        tax_amount_minor: order.tax_amount_minor,
        shipping_amount_minor: order.shipping_amount_minor,
        total_amount_minor: order.total_amount_minor,
        refunded_amount_minor: order.refunded_amount_minor,
        lines: order.lines.into_iter().map(order_line_data).collect(),
        payment_attempt: order.payment_attempt.map(payment_attempt_data),
        refunds: order.refunds.into_iter().map(refund_data).collect(),
        fulfillments: order
            .fulfillments
            .into_iter()
            .map(fulfillment_data)
            .collect(),
        created_at: order.created_at.into(),
        updated_at: order.updated_at.into(),
    })
}

fn tracked_order_data(order: OrderDetail) -> TrackedOrderData {
    let shipping_address = order.identity.shipping_address();
    TrackedOrderData {
        id: order.id.as_uuid(),
        order_number: order.order_number.as_str().into(),
        currency: order.currency.as_str().to_owned(),
        status: order.status.as_str(),
        payment_status: order.payment_status.as_str(),
        shipping_status: order.shipping_status.as_str(),
        shipping_locality: shipping_address.map(|address| address.locality().to_owned()),
        shipping_country_code: shipping_address.map(|address| address.country_code().to_owned()),
        subtotal_amount_minor: order.subtotal_amount_minor,
        discount_amount_minor: order.discount_amount_minor,
        tax_amount_minor: order.tax_amount_minor,
        shipping_amount_minor: order.shipping_amount_minor,
        total_amount_minor: order.total_amount_minor,
        refunded_amount_minor: order.refunded_amount_minor,
        fulfillments: order
            .fulfillments
            .into_iter()
            .map(tracked_fulfillment_data)
            .collect(),
        lines: order.lines.into_iter().map(order_line_data).collect(),
        created_at: order.created_at.into(),
        updated_at: order.updated_at.into(),
    }
}

fn payment_attempt_data(item: OrderPaymentAttemptItem) -> PaymentAttemptData {
    PaymentAttemptData {
        status: item.status.as_str(),
        amount_minor: item.amount_minor,
        provider_reference_id: item.provider_reference_id,
        failure_code: item.failure_code,
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    }
}

fn refund_data(item: OrderRefundItem) -> RefundData {
    RefundData {
        id: item.id.as_uuid(),
        status: item.status.as_str(),
        amount_minor: item.amount_minor,
        provider_reference_id: item.provider_reference_id,
        failure_code: item.failure_code,
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    }
}

fn fulfillment_data(item: OrderFulfillmentItem) -> FulfillmentData {
    FulfillmentData {
        id: item.id.as_uuid(),
        status: item.status.as_str(),
        tracking_number: item.tracking_number,
        tracking_url: item.tracking_url,
        shipped_at: item.shipped_at.map(Into::into),
        delivered_at: item.delivered_at.map(Into::into),
        cancelled_at: item.cancelled_at.map(Into::into),
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    }
}

fn tracked_fulfillment_data(item: OrderFulfillmentItem) -> TrackedFulfillmentData {
    TrackedFulfillmentData {
        status: item.status.as_str(),
        tracking_number: item.tracking_number,
        tracking_url: item.tracking_url,
        shipped_at: item.shipped_at.map(Into::into),
        delivered_at: item.delivered_at.map(Into::into),
    }
}

fn order_line_data(line: OrderLineItem) -> OrderLineData {
    OrderLineData {
        product_id: line.product_id.as_uuid(),
        product_variant_id: line.product_variant_id.as_uuid(),
        product_title: line.product_title,
        variant_title: line.variant_title,
        sku: line.sku,
        track_inventory: line.track_inventory,
        quantity: line.quantity,
        unit_price_amount_minor: line.unit_price_amount_minor,
        subtotal_amount_minor: line.subtotal_amount_minor,
    }
}
