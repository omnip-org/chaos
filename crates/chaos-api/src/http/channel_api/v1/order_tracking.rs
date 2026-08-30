use axum::{Router, extract::State, routing::post};
use chaos_core::contracts::{OrderDetail, OrderFulfillmentItem, OrderLineItem};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{ApiDateTime, ApiError, ApiJson, ApiResponse, ApiState, PublishableChannel};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new().route("/orders/tracking", post(get_tracked_order))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackingTokenBody {
    tracking_token: String,
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

async fn get_tracked_order(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiJson(body): ApiJson<TrackingTokenBody>,
) -> Result<ApiResponse<TrackedOrderData>, ApiError> {
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
