use chaos_core::payments::CreateRefundInput;
use chaos_domain::sales::OrderId;
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

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateRefundParams {
    /// The Store UUID containing the order.
    pub store_id: String,
    /// The Order's UUID.
    pub order_id: String,
    /// The refund amount in the payment's smallest currency unit (e.g. cents for USD).
    pub amount_minor: i64,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = payments_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Refund some or all of an Order's captured payment in the selected Store. \
                        Requires confirm: true."
    )]
    async fn create_refund(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateRefundParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.access_key_authentication,
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
        match self
            .state
            .payment_service
            .create_refund(CreateRefundInput {
                actor,
                store_id,
                order_id,
                amount_minor: params.amount_minor,
            })
            .await
        {
            Ok(detail) => Ok(text_result(json!({
                "id": detail.id.as_uuid(),
                "order_id": detail.order_id.as_uuid(),
                "amount_minor": detail.amount_minor,
                "currency": detail.currency.as_str(),
                "status": detail.status.as_str(),
                "provider_reference_id": detail.provider_reference_id,
                "failure_code": detail.failure_code,
                "created_at": format_time(detail.created_at),
                "updated_at": format_time(detail.updated_at),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
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
