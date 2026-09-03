use axum::{Router, extract::State, routing::post};
use chaos_core::contracts::{OrderDetail, OrderFulfillmentItem, OrderLineItem};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{ApiDateTime, ApiError, ApiJson, ApiResponse, ApiState, PublishableChannel};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new().route("/orders/lookup", post(lookup_order))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderLookupBody {
    email: String,
    order_number: String,
}

#[derive(Serialize)]
struct OrderLineData {
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

/// The subset of a Fulfillment safe to expose on the order-lookup view:
/// shipping progress and carrier tracking, without the internal Store
/// provider-account id.
#[derive(Serialize)]
struct OrderLookupFulfillmentData {
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

/// The order-lookup view returned for a matching order-number + email pair.
/// Contact details and the full billing/shipping address are deliberately
/// left out — the shopper already has that in the confirmation email, and a
/// low-entropy `(order number, email)` pair is a weaker credential than the
/// old capability link.
#[derive(Serialize)]
struct OrderLookupData {
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
    fulfillments: Vec<OrderLookupFulfillmentData>,
    lines: Vec<OrderLineData>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

async fn lookup_order(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiJson(body): ApiJson<OrderLookupBody>,
) -> Result<ApiResponse<OrderLookupData>, ApiError> {
    let order = state
        .storefront_sales
        .lookup_order(&actor, body.order_number.trim(), &body.email)
        .await?;
    Ok(ApiResponse::ok(order_lookup_data(order)))
}

fn order_lookup_data(order: OrderDetail) -> OrderLookupData {
    let shipping_address = order.identity.shipping_address();
    OrderLookupData {
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
            .map(order_lookup_fulfillment_data)
            .collect(),
        lines: order.lines.into_iter().map(order_line_data).collect(),
        created_at: order.created_at.into(),
        updated_at: order.updated_at.into(),
    }
}

fn order_lookup_fulfillment_data(item: OrderFulfillmentItem) -> OrderLookupFulfillmentData {
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

#[cfg(test)]
mod tests {
    use chaos_domain::{
        CurrencyCode,
        fulfillment::FulfillmentStatus,
        pricing::PriceListId,
        sales::{
            OrderContact, OrderId, OrderIdentity, OrderNumber, OrderPaymentStatus, OrderStatus,
            PostalAddress, ShopperId,
        },
    };
    use uuid::Uuid;

    use super::{OrderDetail, order_lookup_data};

    fn sample_order() -> OrderDetail {
        let contact = OrderContact::new(Some("buyer@example.com"), Some("+14155552671".into()))
            .expect("contact");
        let address = PostalAddress::new(
            "Jane Buyer",
            "1 Market Street",
            None,
            "San Francisco",
            Some("CA".into()),
            Some("94105".into()),
            "US",
        )
        .expect("address");
        OrderDetail {
            id: OrderId::from_uuid(Uuid::now_v7()),
            order_number: OrderNumber::parse("W-20260903-7K4M9Q2D").expect("number"),
            shopper_id: ShopperId::from_uuid(Uuid::now_v7()),
            price_list_id: PriceListId::from_uuid(Uuid::now_v7()),
            currency: CurrencyCode::parse("USD").expect("currency"),
            status: OrderStatus::parse("confirmed").expect("status"),
            payment_status: OrderPaymentStatus::parse("paid").expect("payment status"),
            shipping_status: FulfillmentStatus::parse("awaiting_pickup").expect("shipping status"),
            payment_provider: None,
            payment_provider_reference_id: None,
            identity: OrderIdentity::new(contact, Some(address.clone()), Some(address)),
            subtotal_amount_minor: 1300,
            discount_amount_minor: 0,
            tax_amount_minor: 50,
            shipping_amount_minor: 99,
            total_amount_minor: 1449,
            amounts_finalized_at: None,
            refunded_amount_minor: 0,
            lines: Vec::new(),
            payment_attempt: None,
            refunds: Vec::new(),
            fulfillments: Vec::new(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn lookup_view_exposes_the_order_number_and_shipping_region_only() {
        let value = serde_json::to_value(order_lookup_data(sample_order())).expect("serialize");
        let object = value.as_object().expect("object");

        assert_eq!(object["order_number"], "W-20260903-7K4M9Q2D");
        assert_eq!(object["shipping_locality"], "San Francisco");
        assert_eq!(object["shipping_country_code"], "US");
    }

    #[test]
    fn lookup_view_never_echoes_contact_details_or_the_full_address() {
        let value = serde_json::to_value(order_lookup_data(sample_order())).expect("serialize");
        let rendered = value.to_string();

        for leaked in [
            "buyer@example.com",
            "+14155552671",
            "1 Market Street",
            "Jane Buyer",
            "94105",
            "contact_email",
            "billing",
            "address_line1",
        ] {
            assert!(
                !rendered.contains(leaked),
                "order lookup response must not contain {leaked}"
            );
        }
    }
}
