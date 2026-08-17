use chaos_application::ports::{AdminActor, OrderListFilter};
use chaos_domain::{merchant::ApiKeyScope, sales::OrderId};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use time::format_description::well_known::Rfc3339;

use super::ChaosMcp;
use crate::error::{text_result, tool_error};

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
}

#[derive(Deserialize, JsonSchema)]
pub struct GetOrderParams {
    /// The order's UUID.
    pub order_id: String,
}

#[tool_router(router = orders_tool_router, vis = "pub(super)")]
impl ChaosMcp {
    #[tool(
        description = "List orders in the Store bound to this API key. Paginated; use the \
                        returned next_cursor for more pages."
    )]
    async fn list_orders(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListOrdersParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::OrdersRead,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
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
                    status,
                    customer_id: None,
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

    #[tool(description = "Get a single order's summary in the Store bound to this API key.")]
    async fn get_order(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetOrderParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::OrdersRead,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
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
}

fn order_summary(detail: chaos_application::ports::OrderDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "status": detail.status.as_str(),
        "fulfillment_status": detail.fulfillment_status.as_str(),
        "delivery_status": detail.delivery_status.as_str(),
        "currency": detail.currency.as_str(),
        "subtotal_amount_minor": detail.subtotal_amount_minor,
        "discount_amount_minor": detail.discount_amount_minor,
        "tax_amount_minor": detail.tax_amount_minor,
        "shipping_amount_minor": detail.shipping_amount_minor,
        "total_amount_minor": detail.total_amount_minor,
        "line_count": detail.lines.len(),
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
