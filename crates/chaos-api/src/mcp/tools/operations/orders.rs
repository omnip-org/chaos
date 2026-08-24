use chaos_core::{contracts::OrderListFilter, sales::ChangeOrderStatusInput};
use chaos_domain::sales::{OrderId, OrderStatus};
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
pub struct ListOrdersParams {
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of orders to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
    /// Filter by order status: pending, confirmed, or cancelled.
    #[serde(default)]
    pub status: Option<String>,
    /// Exact customer-facing Order number, for example W-20260820-7K4M9Q2D.
    #[serde(default)]
    pub order_number: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetOrderParams {
    /// The order's UUID.
    pub order_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeOrderStatusParams {
    /// The order's UUID.
    pub order_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = orders_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(description = "List orders in the selected Store. Paginated; use the \
                        returned next_cursor for more pages.")]
    async fn list_orders(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListOrdersParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.access_key_authentication,
            &self.state.store_queries,
            &parts,
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
        let status = match params.status.as_deref().map(parse_order_status) {
            Some(Ok(status)) => Some(status),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
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
                    .map(order_summary)
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

    #[tool(description = "Get a single order's summary in the selected Store.")]
    async fn get_order(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetOrderParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.access_key_authentication,
            &self.state.store_queries,
            &parts,
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
            Ok(detail) => Ok(text_result(order_summary(detail))),
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
            &self.state.access_key_authentication,
            &self.state.store_queries,
            &parts,
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
            Ok(detail) => Ok(text_result(order_summary(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn order_summary(detail: chaos_core::contracts::OrderDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "order_number": detail.order_number.as_str(),
        "status": detail.status.as_str(),
        "payment_status": detail.payment_status.as_str(),
        "shipping_status": detail.shipping_status.as_str(),
        "payment_provider": detail.payment_provider.map(|value| value.as_str()),
        "payment_provider_reference_id": detail.payment_provider_reference_id,
        "shipping_provider": detail.shipping_provider.map(|value| value.as_str()),
        "shipping_provider_reference_id": detail.shipping_provider_reference_id,
        "currency": detail.currency.as_str(),
        "subtotal_amount_minor": detail.subtotal_amount_minor,
        "discount_amount_minor": detail.discount_amount_minor,
        "tax_amount_minor": detail.tax_amount_minor,
        "shipping_amount_minor": detail.shipping_amount_minor,
        "total_amount_minor": detail.total_amount_minor,
        "refunded_amount_minor": detail.refunded_amount_minor,
        "lines": detail.lines.into_iter().map(|line| json!({
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
        "payment_attempt": detail.payment_attempt.map(|attempt| json!({
            "status": attempt.status.as_str(),
            "amount_minor": attempt.amount_minor,
            "provider_reference_id": attempt.provider_reference_id,
            "failure_code": attempt.failure_code,
            "created_at": format_time(attempt.created_at),
            "updated_at": format_time(attempt.updated_at),
        })),
        "refunds": detail.refunds.into_iter().map(|refund| json!({
            "id": refund.id.as_uuid(),
            "status": refund.status.as_str(),
            "amount_minor": refund.amount_minor,
            "provider_reference_id": refund.provider_reference_id,
            "failure_code": refund.failure_code,
            "created_at": format_time(refund.created_at),
            "updated_at": format_time(refund.updated_at),
        })).collect::<Vec<_>>(),
        "fulfillments": detail.fulfillments.into_iter().map(|fulfillment| json!({
            "id": fulfillment.id.as_uuid(),
            "shipping_provider_account_id": fulfillment.shipping_provider_account_id.as_uuid(),
            "status": fulfillment.status.as_str(),
            "tracking_number": fulfillment.tracking_number,
            "tracking_url": fulfillment.tracking_url,
            "shipped_at": fulfillment.shipped_at.map(format_time),
            "delivered_at": fulfillment.delivered_at.map(format_time),
            "cancelled_at": fulfillment.cancelled_at.map(format_time),
            "created_at": format_time(fulfillment.created_at),
            "updated_at": format_time(fulfillment.updated_at),
        })).collect::<Vec<_>>(),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
}

fn parse_order_status(value: &str) -> Result<chaos_domain::sales::OrderStatus, CallToolResult> {
    chaos_domain::sales::OrderStatus::parse(value).ok_or_else(|| {
        CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": "status must be one of: pending, confirmed, cancelled",
        }))
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
