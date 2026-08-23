use chaos_application::inventory::AdjustInventoryInput;
use chaos_domain::catalog::ProductVariantId;
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
pub struct ListVariantInventoryParams {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AdjustInventoryParams {
    /// The product variant's UUID.
    pub product_variant_id: String,
    /// Signed change to on-hand quantity. Positive receives inventory; negative removes it.
    pub delta_quantity: i64,
    /// A short human-readable reason for this adjustment (1-500 characters).
    pub note: String,
    pub confirm: bool,
}

#[tool_router(router = inventory_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List on-hand inventory quantities for tracked product variants in the selected Store."
    )]
    async fn list_variant_inventory(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListVariantInventoryParams>,
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
        let after = match params.cursor.as_deref().map(parse_product_variant_cursor) {
            Some(Ok(id)) => Some(id),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let store_id = actor.store_id();
        match self
            .state
            .inventory_management
            .list_variant_inventory(actor, store_id, after, params.limit.unwrap_or(20))
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(|item| {
                        json!({
                            "product_variant_id": item.product_variant_id.as_uuid(),
                            "on_hand_quantity": item.on_hand_quantity,
                            "updated_at": format_time(item.updated_at),
                        })
                    })
                    .collect::<Vec<_>>();
                let next_cursor = page
                    .has_more
                    .then(|| {
                        items
                            .last()
                            .and_then(|item| item["product_variant_id"].as_str().map(String::from))
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
        description = "Adjust the on-hand quantity for one product variant in the selected Store. Requires confirm: true."
    )]
    async fn adjust_variant_inventory(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AdjustInventoryParams>,
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
        let product_variant_id =
            match parse_uuid_field(&params.product_variant_id, "product_variant_id") {
                Ok(id) => ProductVariantId::from_uuid(id),
                Err(result) => return Ok(result),
            };
        let store_id = actor.store_id();
        match self
            .state
            .inventory_management
            .adjust_variant_inventory(AdjustInventoryInput {
                actor,
                store_id,
                product_variant_id,
                delta_quantity: params.delta_quantity,
                note: params.note,
            })
            .await
        {
            Ok(item) => Ok(text_result(json!({
                "product_variant_id": item.product_variant_id.as_uuid(),
                "on_hand_quantity": item.on_hand_quantity,
                "updated_at": format_time(item.updated_at),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn parse_product_variant_cursor(value: &str) -> Result<ProductVariantId, CallToolResult> {
    parse_uuid_field(value, "cursor").map(ProductVariantId::from_uuid)
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
