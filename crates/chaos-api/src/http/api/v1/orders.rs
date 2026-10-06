//! Storefront order search and shopper-owned order details.

use axum::{Router, extract::State, http::header, response::IntoResponse, routing::get};
use chaos_core::contracts::{OrderDetail, OrderFulfillmentItem, OrderLineItem};
use chaos_domain::sales::OrderId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiDateTime, ApiError, ApiPath, ApiQuery, ApiResponse, ApiState, PublishableChannel,
    ShopperContext,
};

use super::wire::{FulfillmentStatus, OrderPaymentStatus, OrderStatus};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/orders/search", get(lookup_order))
        .route("/orders/{order_id}/details", get(get_own_order))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderSearchQuery {
    email: String,
    order_number: String,
}

#[derive(Deserialize)]
struct OrderPath {
    order_id: Uuid,
}

#[derive(Serialize)]
struct OrderLineResponse {
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

impl From<OrderLineItem> for OrderLineResponse {
    fn from(line: OrderLineItem) -> Self {
        Self {
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

#[derive(Serialize)]
struct OrderFulfillmentResponse {
    status: FulfillmentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracking_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracking_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipped_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivered_at: Option<ApiDateTime>,
}

impl From<OrderFulfillmentItem> for OrderFulfillmentResponse {
    fn from(item: OrderFulfillmentItem) -> Self {
        Self {
            status: item.status.into(),
            tracking_number: item.tracking_number,
            tracking_url: item.tracking_url,
            shipped_at: item.shipped_at.map(Into::into),
            delivered_at: item.delivered_at.map(Into::into),
        }
    }
}

#[derive(Serialize)]
struct OrderResponse {
    id: Uuid,
    order_number: String,
    currency: String,
    status: OrderStatus,
    payment_status: OrderPaymentStatus,
    fulfillment_status: FulfillmentStatus,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    refunded_amount_minor: i64,
    fulfillments: Vec<OrderFulfillmentResponse>,
    lines: Vec<OrderLineResponse>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

impl From<OrderDetail> for OrderResponse {
    fn from(order: OrderDetail) -> Self {
        Self {
            id: order.id.as_uuid(),
            order_number: order.order_number.as_str().into(),
            currency: order.currency.as_str().to_owned(),
            status: order.status.into(),
            payment_status: order.payment_status.into(),
            fulfillment_status: order.fulfillment_status.into(),
            subtotal_amount_minor: order.subtotal_amount_minor,
            discount_amount_minor: order.discount_amount_minor,
            tax_amount_minor: order.tax_amount_minor,
            shipping_amount_minor: order.shipping_amount_minor,
            total_amount_minor: order.total_amount_minor,
            refunded_amount_minor: order.refunded_amount_minor,
            fulfillments: order.fulfillments.into_iter().map(Into::into).collect(),
            lines: order.lines.into_iter().map(Into::into).collect(),
            created_at: order.created_at.into(),
            updated_at: order.updated_at.into(),
        }
    }
}

#[derive(Serialize)]
struct OrderLookupResponse {
    #[serde(flatten)]
    order: OrderResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_country_code: Option<String>,
}

impl From<OrderDetail> for OrderLookupResponse {
    fn from(order: OrderDetail) -> Self {
        let shipping_address = order.identity.shipping_address();
        let shipping_locality = shipping_address.map(|address| address.locality().to_owned());
        let shipping_country_code =
            shipping_address.map(|address| address.country_code().to_owned());

        Self {
            order: order.into(),
            shipping_locality,
            shipping_country_code,
        }
    }
}

#[derive(Serialize)]
struct OwnOrderResponse {
    #[serde(flatten)]
    order: OrderResponse,
    contact_email: Option<String>,
    contact_phone: Option<String>,
    billing_full_name: Option<String>,
    billing_address_line1: Option<String>,
    billing_address_line2: Option<String>,
    billing_locality: Option<String>,
    billing_administrative_area: Option<String>,
    billing_postal_code: Option<String>,
    billing_country_code: Option<String>,
    shipping_full_name: Option<String>,
    shipping_address_line1: Option<String>,
    shipping_address_line2: Option<String>,
    shipping_locality: Option<String>,
    shipping_administrative_area: Option<String>,
    shipping_postal_code: Option<String>,
    shipping_country_code: Option<String>,
}

impl From<OrderDetail> for OwnOrderResponse {
    fn from(detail: OrderDetail) -> Self {
        let contact_email = detail.identity.contact().email().map(str::to_owned);
        let contact_phone = detail.identity.contact().phone().map(str::to_owned);
        let billing_address = detail.identity.billing_address();
        let billing_full_name = billing_address.map(|address| address.full_name().to_owned());
        let billing_address_line1 =
            billing_address.map(|address| address.address_line1().to_owned());
        let billing_address_line2 = billing_address
            .and_then(|address| address.address_line2())
            .map(str::to_owned);
        let billing_locality = billing_address.map(|address| address.locality().to_owned());
        let billing_administrative_area = billing_address
            .and_then(|address| address.administrative_area())
            .map(str::to_owned);
        let billing_postal_code = billing_address
            .and_then(|address| address.postal_code())
            .map(str::to_owned);
        let billing_country_code = billing_address.map(|address| address.country_code().to_owned());
        let shipping_address = detail.identity.shipping_address();
        let shipping_full_name = shipping_address.map(|address| address.full_name().to_owned());
        let shipping_address_line1 =
            shipping_address.map(|address| address.address_line1().to_owned());
        let shipping_address_line2 = shipping_address
            .and_then(|address| address.address_line2())
            .map(str::to_owned);
        let shipping_locality = shipping_address.map(|address| address.locality().to_owned());
        let shipping_administrative_area = shipping_address
            .and_then(|address| address.administrative_area())
            .map(str::to_owned);
        let shipping_postal_code = shipping_address
            .and_then(|address| address.postal_code())
            .map(str::to_owned);
        let shipping_country_code =
            shipping_address.map(|address| address.country_code().to_owned());

        Self {
            order: detail.into(),
            contact_email,
            contact_phone,
            billing_full_name,
            billing_address_line1,
            billing_address_line2,
            billing_locality,
            billing_administrative_area,
            billing_postal_code,
            billing_country_code,
            shipping_full_name,
            shipping_address_line1,
            shipping_address_line2,
            shipping_locality,
            shipping_administrative_area,
            shipping_postal_code,
            shipping_country_code,
        }
    }
}

async fn lookup_order(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiQuery(query): ApiQuery<OrderSearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let order = state
        .storefront_sales
        .lookup_order(&actor, query.order_number.trim(), &query.email)
        .await?;
    Ok((
        [(header::REFERRER_POLICY, "no-referrer")],
        ApiResponse::ok(OrderLookupResponse::from(order)).private(),
    ))
}

async fn get_own_order(
    State(state): State<ApiState>,
    ShopperContext(shopper): ShopperContext,
    ApiPath(path): ApiPath<OrderPath>,
) -> Result<impl IntoResponse, ApiError> {
    let order = state
        .storefront_sales
        .get_shopper_order(&shopper, OrderId::from_uuid(path.order_id))
        .await?;
    Ok(ApiResponse::ok(OwnOrderResponse::from(order)).private())
}

#[cfg(test)]
mod tests {
    use chaos_core::contracts::OrderDetail;
    use chaos_domain::{
        CurrencyCode,
        fulfillment::FulfillmentStatus as DomainFulfillmentStatus,
        pricing::PriceListId,
        sales::{
            OrderContact, OrderIdentity, OrderNumber,
            OrderPaymentStatus as DomainOrderPaymentStatus, OrderStatus as DomainOrderStatus,
            ShopperId,
        },
    };
    use serde_json::Value;
    use time::OffsetDateTime;

    use super::*;

    #[test]
    fn own_order_serializes_validated_statuses_without_leaking_internal_fields() {
        let order_id = Uuid::from_u128(1);
        let response = OwnOrderResponse::from(OrderDetail {
            id: OrderId::from_uuid(order_id),
            order_number: OrderNumber::parse("W-00000000").unwrap(),
            shopper_id: ShopperId::from_uuid(Uuid::from_u128(6)),
            price_list_id: PriceListId::from_uuid(Uuid::from_u128(7)),
            currency: CurrencyCode::parse("USD").unwrap(),
            status: DomainOrderStatus::Confirmed,
            payment_status: DomainOrderPaymentStatus::Paid,
            fulfillment_status: DomainFulfillmentStatus::Pending,
            payment_provider: None,
            payment_provider_reference_id: None,
            identity: OrderIdentity::new(
                OrderContact::new(None::<String>, None).unwrap(),
                None,
                None,
            ),
            subtotal_amount_minor: 1_000,
            discount_amount_minor: 100,
            tax_amount_minor: 80,
            shipping_amount_minor: 20,
            total_amount_minor: 1_000,
            amounts_finalized_at: None,
            refunded_amount_minor: 0,
            lines: Vec::new(),
            payment_attempt: None,
            refunds: Vec::new(),
            fulfillments: Vec::new(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        });

        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["id"], order_id.to_string());
        assert_eq!(json["status"], "confirmed");
        assert_eq!(json["payment_status"], "paid");
        assert_eq!(json["fulfillment_status"], "pending");
        for field in ["contact_email", "billing_full_name", "shipping_locality"] {
            assert_eq!(
                json[field],
                Value::Null,
                "field {field} must remain nullable"
            );
        }
        for internal in [
            "store_id",
            "channel_id",
            "shopper_id",
            "cart_id",
            "price_list_id",
            "payment_provider",
            "payment_provider_account_id",
            "payment_provider_reference_id",
            "payment_failure_code",
            "amounts_finalized_at",
        ] {
            assert!(
                json.get(internal).is_none(),
                "internal field {internal} must not cross the Storefront boundary"
            );
        }
    }
}
