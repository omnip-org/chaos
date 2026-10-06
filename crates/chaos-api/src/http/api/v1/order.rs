//! Storefront order search and shopper-owned order details.

use axum::{Router, extract::State, routing::get};

use crate::http::ApiState;

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/orders/search", get(lookup_order::handler))
        .route("/orders/{order_id}/details", get(own_order::handler))
}

// ===== GET /orders/search =====

mod lookup_order {
    use chaos_core::contracts::{OrderDetail, OrderFulfillmentItem, OrderLineItem};
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    use super::*;
    use axum::{http::header, response::IntoResponse};

    use crate::http::{ApiDateTime, ApiError, ApiQuery, ApiResponse, PublishableChannel};

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct OrderSearchQuery {
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
        ApiQuery(query): ApiQuery<OrderSearchQuery>,
    ) -> Result<impl IntoResponse, ApiError> {
        let order = state
            .storefront_sales
            .lookup_order(&actor, query.order_number.trim(), &query.email)
            .await?;
        Ok((
            [
                (header::CACHE_CONTROL, "private, no-store"),
                (header::REFERRER_POLICY, "no-referrer"),
            ],
            ApiResponse::ok(order_details_data(order)),
        ))
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

    pub(super) fn order_details_fulfillment_data(
        item: OrderFulfillmentItem,
    ) -> OrderLookupFulfillmentData {
        OrderLookupFulfillmentData {
            status: item.status.as_str(),
            tracking_number: item.tracking_number,
            tracking_url: item.tracking_url,
            shipped_at: item.shipped_at.map(Into::into),
            delivered_at: item.delivered_at.map(Into::into),
        }
    }

    pub(super) fn order_line_data(line: OrderLineItem) -> OrderLineData {
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

mod own_order {
    use axum::{http::header, response::IntoResponse};
    use chaos_core::contracts::{ShopperOrderDetail, ShopperOrderRow};
    use serde::Serialize;
    use uuid::Uuid;

    use super::*;
    use crate::http::{ApiError, ApiPath, ApiResponse, ShopperContext};

    #[derive(serde::Deserialize)]
    pub(super) struct OrderPath {
        order_id: Uuid,
    }

    #[derive(Serialize)]
    pub(super) struct OwnOrderData {
        #[serde(flatten)]
        order_row: ShopperOrderRow,
        lines: Vec<lookup_order::OrderLineData>,
        fulfillments: Vec<lookup_order::OrderLookupFulfillmentData>,
    }

    pub(super) async fn handler(
        State(state): State<ApiState>,
        ShopperContext(shopper): ShopperContext,
        ApiPath(path): ApiPath<OrderPath>,
    ) -> Result<impl IntoResponse, ApiError> {
        let order = state
            .storefront_sales
            .get_shopper_order(
                &shopper,
                chaos_domain::sales::OrderId::from_uuid(path.order_id),
            )
            .await?;
        Ok((
            [(header::CACHE_CONTROL, "private, no-store")],
            ApiResponse::ok(own_order_data(order)),
        ))
    }

    fn own_order_data(order: ShopperOrderDetail) -> OwnOrderData {
        OwnOrderData {
            order_row: order.row,
            lines: order
                .detail
                .lines
                .into_iter()
                .map(lookup_order::order_line_data)
                .collect(),
            fulfillments: order
                .detail
                .fulfillments
                .into_iter()
                .map(lookup_order::order_details_fulfillment_data)
                .collect(),
        }
    }
}
