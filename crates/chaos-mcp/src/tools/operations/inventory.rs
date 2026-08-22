use chaos_application::inventory::{AdjustStockInput, CreateInventoryLocationInput};
use chaos_domain::{
    catalog::ProductVariantId,
    inventory::{InventoryItemId, InventoryLocationId},
};
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

use crate::tools::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, JsonSchema)]
pub struct ListStockParams {
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of inventory items to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListInventoryLocationsParams {
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of locations to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateInventoryLocationParams {
    /// URL-safe code, unique within the Store (e.g. "main-warehouse").
    pub code: String,
    pub name: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AdjustStockParams {
    /// The inventory location's UUID.
    pub inventory_location_id: String,
    /// The product variant's UUID. The variant must have track_inventory enabled.
    pub product_variant_id: String,
    /// Signed change to on-hand quantity (positive to receive stock, negative to remove it).
    /// Must not be zero.
    pub delta_quantity: i64,
    /// A short human-readable reason for this adjustment (1-500 characters), e.g.
    /// "Initial stock receipt" or "Damaged in transit".
    pub note: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = inventory_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List stock levels (on-hand, reserved, available quantity) per inventory \
                        location and product variant in the selected Store."
    )]
    async fn list_inventory(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListStockParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
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
        let after = match params.cursor.as_deref().map(parse_stock_cursor) {
            Some(Ok(id)) => Some(id),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let limit = params.limit.unwrap_or(20);

        match self
            .state
            .inventory_management
            .list_stock(actor, store_id, after, limit)
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(|item| {
                        json!({
                            "id": item.id.as_uuid(),
                            "inventory_location_id": item.inventory_location_id.as_uuid(),
                            "product_variant_id": item.product_variant_id.as_uuid(),
                            "on_hand_quantity": item.on_hand_quantity,
                            "reserved_quantity": item.reserved_quantity,
                            "available_quantity": item.available_quantity,
                            "updated_at": format_time(item.updated_at),
                        })
                    })
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
        description = "List inventory locations (warehouses, stores, etc.) in the selected Store."
    )]
    async fn list_inventory_locations(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListInventoryLocationsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
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
        let after = match params.cursor.as_deref().map(parse_location_cursor) {
            Some(Ok(id)) => Some(id),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let limit = params.limit.unwrap_or(20);

        match self
            .state
            .inventory_management
            .list_locations(actor, store_id, after, limit)
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(|item| {
                        json!({
                            "id": item.id.as_uuid(),
                            "code": item.code,
                            "name": item.name,
                            "archived_at": item.archived_at.map(format_time),
                            "created_at": format_time(item.created_at),
                            "updated_at": format_time(item.updated_at),
                        })
                    })
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
        description = "Create an inventory location (warehouse, store, etc.) in the selected Store. Requires confirm: true and an idempotency_key."
    )]
    async fn create_inventory_location(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateInventoryLocationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
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
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .inventory_management
            .create_location(CreateInventoryLocationInput {
                actor,
                store_id,
                code: params.code,
                name: params.name,
                idempotency,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Adjust on-hand stock quantity for a product variant at an inventory \
                        location, in the selected Store. Use a positive \
                        delta_quantity to receive stock and a negative one to remove it. \
                        Requires confirm: true and an idempotency_key."
    )]
    async fn adjust_stock(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AdjustStockParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_mcp(
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
        let inventory_location_id =
            match parse_uuid_field(&params.inventory_location_id, "inventory_location_id") {
                Ok(id) => InventoryLocationId::from_uuid(id),
                Err(result) => return Ok(result),
            };
        let product_variant_id =
            match parse_uuid_field(&params.product_variant_id, "product_variant_id") {
                Ok(id) => ProductVariantId::from_uuid(id),
                Err(result) => return Ok(result),
            };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .inventory_management
            .adjust_stock(AdjustStockInput {
                actor,
                store_id,
                inventory_location_id,
                product_variant_id,
                delta_quantity: params.delta_quantity,
                note: params.note,
                idempotency,
            })
            .await
        {
            Ok(item) => Ok(text_result(json!({
                "id": item.id.as_uuid(),
                "inventory_location_id": item.inventory_location_id.as_uuid(),
                "product_variant_id": item.product_variant_id.as_uuid(),
                "on_hand_quantity": item.on_hand_quantity,
                "reserved_quantity": item.reserved_quantity,
                "available_quantity": item.available_quantity,
                "updated_at": format_time(item.updated_at),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn parse_stock_cursor(value: &str) -> Result<InventoryItemId, CallToolResult> {
    parse_uuid_field(value, "cursor").map(InventoryItemId::from_uuid)
}

fn parse_location_cursor(value: &str) -> Result<InventoryLocationId, CallToolResult> {
    parse_uuid_field(value, "cursor").map(InventoryLocationId::from_uuid)
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
