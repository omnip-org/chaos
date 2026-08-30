use chaos_core::{
    contracts::{OrderDetail, OrderListFilter},
    sales::ChangeOrderStatusInput,
};
use chaos_domain::sales::{OrderContact, OrderId, OrderStatus, PostalAddress};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::mcp::tools::ChaosMcp;
use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
};

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatusParam {
    Pending,
    Confirmed,
    Cancelled,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListOrdersParams {
    /// The Store UUID to inspect.
    pub store_id: String,
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of orders to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
    /// Filter by order status.
    #[serde(default)]
    pub status: Option<OrderStatusParam>,
    /// Exact customer-facing Order number, for example W-20260820-7K4M9Q2D.
    #[serde(default)]
    pub order_number: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetOrderParams {
    /// The Store UUID containing the order.
    pub store_id: String,
    /// The order's UUID.
    pub order_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeOrderStatusParams {
    /// The Store UUID containing the order.
    pub store_id: String,
    /// The order's UUID.
    pub order_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = orders_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List order summaries in the selected Store. Paginated; use the \
                        returned next_cursor for more pages. Call get_order for full customer, \
                        address, line, payment, refund, and fulfillment details."
    )]
    async fn list_orders(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListOrdersParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
            &self.state.store_queries,
            &parts,
            &params.store_id,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let after = match params.cursor.as_deref().map(parse_uuid_cursor) {
            Some(Ok(id)) => Some(id),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let status = params.status.map(|status| match status {
            OrderStatusParam::Pending => OrderStatus::Pending,
            OrderStatusParam::Confirmed => OrderStatus::Confirmed,
            OrderStatusParam::Cancelled => OrderStatus::Cancelled,
        });
        let limit = params.limit.unwrap_or(20);

        match self
            .state
            .order_management
            .list_orders(
                actor,
                store_id,
                after,
                limit,
                OrderListFilter {
                    order_number: params.order_number,
                    status,
                    email: None,
                },
            )
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(order_list_item)
                    .collect::<Vec<_>>();
                let next_cursor = page
                    .has_more
                    .then(|| {
                        items
                            .last()
                            .and_then(|item| item["id"].as_str().map(String::from))
                    })
                    .flatten();
                Ok(text_result(json!({
                    "items": items,
                    "has_more": page.has_more,
                    "next_cursor": next_cursor,
                })))
            }
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get a single order's full detail in the selected Store, including \
                        customer contact data, billing and shipping addresses, line items, \
                        payment attempts, refunds, and fulfillments."
    )]
    async fn get_order(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetOrderParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
            &self.state.store_queries,
            &parts,
            &params.store_id,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let order_id = match parse_uuid_field(&params.order_id, "order_id") {
            Ok(id) => OrderId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .order_management
            .get_order(actor, store_id, order_id)
            .await
        {
            Ok(detail) => Ok(text_result(order_detail(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Confirm a pending order in the selected Store. Requires \
                        confirm: true."
    )]
    async fn confirm_order(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeOrderStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_order_status(parts, params, OrderStatus::Confirmed)
            .await
    }

    #[tool(
        description = "Cancel a pending order in the selected Store. Requires \
                        confirm: true."
    )]
    async fn cancel_order(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeOrderStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_order_status(parts, params, OrderStatus::Cancelled)
            .await
    }
}

impl ChaosMcp {
    async fn change_order_status(
        &self,
        parts: http::request::Parts,
        params: ChangeOrderStatusParams,
        target_status: OrderStatus,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
            &self.state.store_queries,
            &parts,
            &params.store_id,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let store_id = actor.store_id();
        let order_id = match parse_uuid_field(&params.order_id, "order_id") {
            Ok(id) => OrderId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let now = self.state.clock.now();

        match self
            .state
            .order_management
            .change_status(ChangeOrderStatusInput {
                actor,
                store_id,
                order_id,
                target_status,
                now,
            })
            .await
        {
            Ok(detail) => Ok(text_result(order_detail(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn order_list_item(detail: OrderDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "order_number": detail.order_number.as_str(),
        "shopper_id": detail.shopper_id.as_uuid(),
        "status": detail.status.as_str(),
        "payment_status": detail.payment_status.as_str(),
        "shipping_status": detail.shipping_status.as_str(),
        "currency": detail.currency.as_str(),
        "total_amount_minor": detail.total_amount_minor,
        "amounts_finalized_at": detail.amounts_finalized_at.map(format_time),
        "refunded_amount_minor": detail.refunded_amount_minor,
        "contact_email": detail.identity.contact().email(),
        "line_count": detail.lines.len(),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn order_detail(detail: OrderDetail) -> serde_json::Value {
    let OrderDetail {
        id,
        order_number,
        shopper_id,
        price_list_id,
        currency,
        status,
        payment_status,
        shipping_status,
        payment_provider,
        payment_provider_reference_id,
        identity,
        subtotal_amount_minor,
        discount_amount_minor,
        tax_amount_minor,
        shipping_amount_minor,
        total_amount_minor,
        amounts_finalized_at,
        refunded_amount_minor,
        lines,
        payment_attempt,
        refunds,
        fulfillments,
        created_at,
        updated_at,
    } = detail;

    json!({
        "id": id.as_uuid(),
        "order_number": order_number.as_str(),
        "shopper_id": shopper_id.as_uuid(),
        "price_list_id": price_list_id.as_uuid(),
        "status": status.as_str(),
        "payment_status": payment_status.as_str(),
        "shipping_status": shipping_status.as_str(),
        "payment_provider": payment_provider.map(|value| value.as_str()),
        "payment_provider_reference_id": payment_provider_reference_id,
        "contact": order_contact_data(identity.contact()),
        "billing_address": identity.billing_address().map(postal_address_data),
        "shipping_address": identity.shipping_address().map(postal_address_data),
        "currency": currency.as_str(),
        "subtotal_amount_minor": subtotal_amount_minor,
        "discount_amount_minor": discount_amount_minor,
        "tax_amount_minor": tax_amount_minor,
        "shipping_amount_minor": shipping_amount_minor,
        "total_amount_minor": total_amount_minor,
        "amounts_finalized_at": amounts_finalized_at.map(format_time),
        "refunded_amount_minor": refunded_amount_minor,
        "lines": lines.into_iter().map(|line| json!({
            "product_id": line.product_id.as_uuid(),
            "product_variant_id": line.product_variant_id.as_uuid(),
            "product_title": line.product_title,
            "variant_title": line.variant_title,
            "sku": line.sku,
            "track_inventory": line.track_inventory,
            "quantity": line.quantity,
            "unit_price_amount_minor": line.unit_price_amount_minor,
            "subtotal_amount_minor": line.subtotal_amount_minor,
        })).collect::<Vec<_>>(),
        "payment_attempt": payment_attempt.map(|attempt| json!({
            "status": attempt.status.as_str(),
            "amount_minor": attempt.amount_minor,
            "provider_reference_id": attempt.provider_reference_id,
            "failure_code": attempt.failure_code,
            "created_at": format_time(attempt.created_at),
            "updated_at": format_time(attempt.updated_at),
        })),
        "refunds": refunds.into_iter().map(|refund| json!({
            "id": refund.id.as_uuid(),
            "status": refund.status.as_str(),
            "amount_minor": refund.amount_minor,
            "provider_reference_id": refund.provider_reference_id,
            "failure_code": refund.failure_code,
            "created_at": format_time(refund.created_at),
            "updated_at": format_time(refund.updated_at),
        })).collect::<Vec<_>>(),
        "fulfillments": fulfillments.into_iter().map(|fulfillment| json!({
            "id": fulfillment.id.as_uuid(),
            "shipping_provider_account_id": fulfillment.shipping_provider_account_id.as_uuid(),
            "shipping_provider": fulfillment.shipping_provider.as_str(),
            "provider_reference_id": fulfillment.provider_reference_id,
            "status": fulfillment.status.as_str(),
            "tracking_number": fulfillment.tracking_number,
            "tracking_url": fulfillment.tracking_url,
            "shipped_at": fulfillment.shipped_at.map(format_time),
            "delivered_at": fulfillment.delivered_at.map(format_time),
            "cancelled_at": fulfillment.cancelled_at.map(format_time),
            "created_at": format_time(fulfillment.created_at),
            "updated_at": format_time(fulfillment.updated_at),
        })).collect::<Vec<_>>(),
        "created_at": format_time(created_at),
        "updated_at": format_time(updated_at),
    })
}

fn order_contact_data(contact: &OrderContact) -> serde_json::Value {
    json!({
        "email": contact.email(),
        "phone": contact.phone(),
    })
}

fn postal_address_data(address: &PostalAddress) -> serde_json::Value {
    json!({
        "full_name": address.full_name(),
        "address_line1": address.address_line1(),
        "address_line2": address.address_line2(),
        "locality": address.locality(),
        "administrative_area": address.administrative_area(),
        "postal_code": address.postal_code(),
        "country_code": address.country_code(),
    })
}

fn parse_uuid_cursor(value: &str) -> Result<uuid::Uuid, CallToolResult> {
    parse_uuid_field(value, "cursor")
}

fn parse_uuid_field(value: &str, field: &'static str) -> Result<uuid::Uuid, CallToolResult> {
    uuid::Uuid::parse_str(value).map_err(|_| {
        CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": format!("{field} must be a valid UUID"),
        }))
    })
}

fn format_time(value: time::OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{order_detail, order_list_item};
    use chaos_core::contracts::OrderDetail;
    use chaos_domain::{
        CurrencyCode,
        fulfillment::FulfillmentStatus,
        pricing::PriceListId,
        sales::{
            OrderContact, OrderId, OrderIdentity, OrderNumber, OrderPaymentStatus, OrderStatus,
            PostalAddress, ShopperId,
        },
    };
    use serde_json::json;
    use time::OffsetDateTime;

    fn sample_order() -> (OrderDetail, uuid::Uuid, uuid::Uuid, uuid::Uuid) {
        let order_id = OrderId::new();
        let shopper_id = ShopperId::new();
        let price_list_id = PriceListId::new();
        let contact =
            OrderContact::new(Some("buyer@example.com"), Some("+14155552671".to_owned())).unwrap();
        let billing_address = PostalAddress::new(
            "Buyer",
            "1 Market Street",
            Some("Suite 100".to_owned()),
            "San Francisco",
            Some("CA".to_owned()),
            Some("94105".to_owned()),
            "US",
        )
        .unwrap();
        let shipping_address = PostalAddress::new(
            "Buyer",
            "2 Market Street",
            None,
            "San Francisco",
            Some("CA".to_owned()),
            Some("94105".to_owned()),
            "US",
        )
        .unwrap();

        (
            OrderDetail {
                id: order_id,
                order_number: OrderNumber::parse("W-20260820-7K4M9Q2D").unwrap(),
                shopper_id,
                price_list_id,
                currency: CurrencyCode::USD,
                status: OrderStatus::Pending,
                payment_status: OrderPaymentStatus::Pending,
                shipping_status: FulfillmentStatus::Pending,
                payment_provider: None,
                payment_provider_reference_id: None,
                identity: OrderIdentity::new(
                    contact,
                    Some(billing_address),
                    Some(shipping_address),
                ),
                subtotal_amount_minor: 1_000,
                discount_amount_minor: 100,
                tax_amount_minor: 90,
                shipping_amount_minor: 50,
                total_amount_minor: 1_040,
                amounts_finalized_at: None,
                refunded_amount_minor: 0,
                lines: Vec::new(),
                payment_attempt: None,
                refunds: Vec::new(),
                fulfillments: Vec::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            },
            order_id.as_uuid(),
            shopper_id.as_uuid(),
            price_list_id.as_uuid(),
        )
    }

    #[test]
    fn full_order_detail_includes_customer_identity() {
        let (order, order_id, shopper_id, price_list_id) = sample_order();
        let value = order_detail(order);

        assert_eq!(value["id"], json!(order_id));
        assert_eq!(value["shopper_id"], json!(shopper_id));
        assert_eq!(value["price_list_id"], json!(price_list_id));
        assert_eq!(value["contact"]["email"], "buyer@example.com");
        assert_eq!(value["contact"]["phone"], "+14155552671");
        assert_eq!(value["billing_address"]["address_line1"], "1 Market Street");
        assert!(value["billing_address"].get("company").is_none());
        assert_eq!(
            value["shipping_address"]["address_line1"],
            "2 Market Street"
        );
        assert_eq!(value["shipping_address"]["country_code"], "US");
    }

    #[test]
    fn order_list_item_omits_nested_operational_details() {
        let (order, _, shopper_id, _) = sample_order();
        let value = order_list_item(order);

        assert_eq!(value["shopper_id"], json!(shopper_id));
        assert_eq!(value["contact_email"], "buyer@example.com");
        assert_eq!(value["line_count"], 0);
        assert!(value.get("contact").is_none());
        assert!(value.get("billing_address").is_none());
        assert!(value.get("lines").is_none());
        assert!(value.get("refunds").is_none());
        assert!(value.get("fulfillments").is_none());
    }
}
