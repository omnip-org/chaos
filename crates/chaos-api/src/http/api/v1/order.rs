//! Guest order lookup endpoint.

use axum::{Router, extract::State, routing::post};

use crate::http::ApiState;

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new().route("/orders/details", post(lookup_order::handler))
}

// ===== POST /orders/details =====

mod lookup_order {
    use chaos_core::contracts::{OrderDetail, OrderFulfillmentItem, OrderLineItem};
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    use super::*;
    use crate::http::{ApiDateTime, ApiError, ApiJson, ApiResponse, PublishableChannel};

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct OrderLookupBody {
        email: String,
        order_number: String,
    }

    #[derive(Serialize)]
    pub(super) struct OrderLineData {
        product_id: Uuid,
        product_variant_id: Uuid,
        product_title: String,
        variant_title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sku: Option<String>,
        quantity: u32,
        unit_price_amount_minor: i64,
        subtotal_amount_minor: i64,
    }

    #[derive(Serialize)]
    pub(super) struct OrderLookupFulfillmentData {
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
    pub(super) struct OrderLookupData {
        id: Uuid,
        order_number: String,
        currency: String,
        status: &'static str,
        payment_status: &'static str,
        fulfillment_status: &'static str,
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
        fulfillments: Vec<OrderLookupFulfillmentData>,
        lines: Vec<OrderLineData>,
        created_at: ApiDateTime,
        updated_at: ApiDateTime,
    }

    pub(super) async fn handler(
        State(state): State<ApiState>,
        PublishableChannel(actor): PublishableChannel,
        ApiJson(body): ApiJson<OrderLookupBody>,
    ) -> Result<ApiResponse<OrderLookupData>, ApiError> {
        let order = state
            .storefront_sales
            .lookup_order(&actor, body.order_number.trim(), &body.email)
            .await?;
        Ok(ApiResponse::ok(order_details_data(order)))
    }

    fn order_details_data(order: OrderDetail) -> OrderLookupData {
        let shipping_address = order.identity.shipping_address();
        OrderLookupData {
            id: order.id.as_uuid(),
            order_number: order.order_number.as_str().into(),
            currency: order.currency.as_str().to_owned(),
            status: order.status.as_str(),
            payment_status: order.payment_status.as_str(),
            fulfillment_status: order.fulfillment_status.as_str(),
            shipping_locality: shipping_address.map(|address| address.locality().to_owned()),
            shipping_country_code: shipping_address
                .map(|address| address.country_code().to_owned()),
            subtotal_amount_minor: order.subtotal_amount_minor,
            discount_amount_minor: order.discount_amount_minor,
            tax_amount_minor: order.tax_amount_minor,
            shipping_amount_minor: order.shipping_amount_minor,
            total_amount_minor: order.total_amount_minor,
            refunded_amount_minor: order.refunded_amount_minor,
            fulfillments: order
                .fulfillments
                .into_iter()
                .map(order_details_fulfillment_data)
                .collect(),
            lines: order.lines.into_iter().map(order_line_data).collect(),
            created_at: order.created_at.into(),
            updated_at: order.updated_at.into(),
        }
    }

    fn order_details_fulfillment_data(item: OrderFulfillmentItem) -> OrderLookupFulfillmentData {
        OrderLookupFulfillmentData {
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
            quantity: line.quantity,
            unit_price_amount_minor: line.unit_price_amount_minor,
            subtotal_amount_minor: line.subtotal_amount_minor,
        }
    }
}
