use chaos_core::fulfillment::{
    CancelFulfillmentInput, CreateFulfillmentInput, MarkDeliveredInput, MarkShippedInput,
};
use chaos_domain::{fulfillment::FulfillmentId, sales::OrderId};
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
pub struct ListShippingProviderAccountsParams {}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateFulfillmentParams {
    /// The order's UUID.
    pub order_id: String,
    /// The shipping provider account's UUID. Use list_shipping_provider_accounts \
    /// to find the Store's "manual" account.
    pub shipping_provider_account_id: String,
    /// Optional carrier tracking number.
    #[serde(default)]
    pub tracking_number: Option<String>,
    /// Optional carrier tracking URL. Must start with https://.
    #[serde(default)]
    pub tracking_url: Option<String>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct MarkFulfillmentShippedParams {
    /// The fulfillment's UUID.
    pub fulfillment_id: String,
    /// Optional carrier tracking number to set or update.
    #[serde(default)]
    pub tracking_number: Option<String>,
    /// Optional carrier tracking URL to set or update. Must start with https://.
    #[serde(default)]
    pub tracking_url: Option<String>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct FulfillmentIdParams {
    /// The fulfillment's UUID.
    pub fulfillment_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = fulfillment_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List the selected Store's shipping provider accounts. Every Store has a \
                        'manual' account created automatically; no carrier integration exists yet."
    )]
    async fn list_shipping_provider_accounts(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<ListShippingProviderAccountsParams>,
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
        match self
            .state
            .fulfillment_management
            .list_shipping_provider_accounts(actor, store_id)
            .await
        {
            Ok(accounts) => Ok(text_result(json!({
                "items": accounts.into_iter().map(|account| json!({
                    "id": account.id.as_uuid(),
                    "provider": account.provider,
                    "display_name": account.display_name,
                    "enabled": account.enabled,
                    "created_at": format_time(account.created_at),
                    "updated_at": format_time(account.updated_at),
                })).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a Fulfillment for an order in the selected Store, starting in \
                        awaiting_pickup status. An order may only have one active (non-cancelled) \
                        Fulfillment at a time. Requires confirm: true."
    )]
    async fn create_fulfillment(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateFulfillmentParams>,
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
        let shipping_provider_account_id = match parse_uuid_field(
            &params.shipping_provider_account_id,
            "shipping_provider_account_id",
        ) {
            Ok(id) => chaos_domain::fulfillment::ShippingProviderAccountId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        match self
            .state
            .fulfillment_management
            .create_fulfillment(CreateFulfillmentInput {
                actor,
                store_id,
                order_id,
                shipping_provider_account_id,
                tracking_number: params.tracking_number,
                tracking_url: params.tracking_url,
            })
            .await
        {
            Ok(detail) => Ok(text_result(fulfillment_summary(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Mark a Fulfillment as shipped (awaiting_pickup -> shipped). Requires \
                        confirm: true."
    )]
    async fn mark_fulfillment_shipped(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<MarkFulfillmentShippedParams>,
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
        let id = match parse_uuid_field(&params.fulfillment_id, "fulfillment_id") {
            Ok(id) => FulfillmentId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let now = self.state.clock.now();
        match self
            .state
            .fulfillment_management
            .mark_shipped(MarkShippedInput {
                actor,
                store_id,
                id,
                tracking_number: params.tracking_number,
                tracking_url: params.tracking_url,
                now,
            })
            .await
        {
            Ok(detail) => Ok(text_result(fulfillment_summary(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Mark a Fulfillment as delivered (shipped -> delivered). Requires \
                        confirm: true."
    )]
    async fn mark_fulfillment_delivered(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<FulfillmentIdParams>,
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
        let id = match parse_uuid_field(&params.fulfillment_id, "fulfillment_id") {
            Ok(id) => FulfillmentId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let now = self.state.clock.now();
        match self
            .state
            .fulfillment_management
            .mark_delivered(MarkDeliveredInput {
                actor,
                store_id,
                id,
                now,
            })
            .await
        {
            Ok(detail) => Ok(text_result(fulfillment_summary(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Cancel a Fulfillment that has not yet been delivered. Requires \
                        confirm: true."
    )]
    async fn cancel_fulfillment(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<FulfillmentIdParams>,
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
        let id = match parse_uuid_field(&params.fulfillment_id, "fulfillment_id") {
            Ok(id) => FulfillmentId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let now = self.state.clock.now();
        match self
            .state
            .fulfillment_management
            .cancel(CancelFulfillmentInput {
                actor,
                store_id,
                id,
                now,
            })
            .await
        {
            Ok(detail) => Ok(text_result(fulfillment_summary(detail))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn fulfillment_summary(detail: chaos_core::contracts::FulfillmentDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "order_id": detail.order_id.as_uuid(),
        "shipping_provider_account_id": detail.shipping_provider_account_id.as_uuid(),
        "status": detail.status.as_str(),
        "tracking_number": detail.tracking_number,
        "tracking_url": detail.tracking_url,
        "shipped_at": detail.shipped_at.map(format_time),
        "delivered_at": detail.delivered_at.map(format_time),
        "cancelled_at": detail.cancelled_at.map(format_time),
        "created_at": format_time(detail.created_at),
        "updated_at": format_time(detail.updated_at),
    })
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
