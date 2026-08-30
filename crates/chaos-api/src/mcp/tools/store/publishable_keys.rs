use chaos_core::store::CreatePublishableKeyInput;
use chaos_domain::store::{PublishableKeyId, SalesChannelId};
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
pub struct CreatePublishableKeyParams {
    /// The Store UUID to modify.
    pub store_id: String,
    /// The active Sales Channel UUID to bind to the key.
    pub sales_channel_id: String,
    pub name: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListPublishableKeysParams {
    /// The Store UUID to inspect.
    pub store_id: String,
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of Publishable Keys to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RevokePublishableKeyParams {
    /// The Store UUID containing the key.
    pub store_id: String,
    /// The Publishable Key's UUID.
    pub publishable_key_id: String,
    /// Must be explicitly set to true. This action is irreversible and affects live
    /// store data — any caller presenting this key is immediately locked out.
    pub confirm: bool,
}

#[tool_router(router = publishable_keys_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Create a public Storefront Key in the selected Store and bind it to an \
                        active Sales Channel. The key is safe to use in frontend code. Requires \
                        confirm: true."
    )]
    async fn create_publishable_key(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreatePublishableKeyParams>,
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
        let sales_channel_id = match parse_uuid_field(&params.sales_channel_id, "sales_channel_id")
        {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        match self
            .state
            .publishable_key_management
            .create(CreatePublishableKeyInput {
                actor,
                store_id,
                sales_channel_id,
                name: params.name,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "id": output.publishable_key.id().as_uuid(),
                "sales_channel_id": output.publishable_key.sales_channel_id().as_uuid(),
                "name": output.publishable_key.name(),
                "public_key": output.public_key,
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List public Storefront Keys in the selected Store. Paginated; use the \
                        returned next_cursor for more \
                        pages."
    )]
    async fn list_publishable_keys(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListPublishableKeysParams>,
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
        let store_id = actor.store_id();
        let after = match params.cursor.as_deref().map(parse_uuid_cursor) {
            Some(Ok(id)) => Some(PublishableKeyId::from_uuid(id)),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let limit = params.limit.unwrap_or(20);

        match self
            .state
            .publishable_key_management
            .list(actor, store_id, after, limit)
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(publishable_key_summary)
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
        description = "Revoke a Publishable Key in the selected Store, immediately \
                        locking out anyone presenting it. Requires confirm: true and an \
                        confirm: true."
    )]
    async fn revoke_publishable_key(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RevokePublishableKeyParams>,
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
        let publishable_key_id =
            match parse_uuid_field(&params.publishable_key_id, "publishable_key_id") {
                Ok(id) => PublishableKeyId::from_uuid(id),
                Err(result) => return Ok(result),
            };
        let store_id = actor.store_id();
        match self
            .state
            .publishable_key_management
            .revoke(actor, store_id, publishable_key_id)
            .await
        {
            Ok(()) => Ok(text_result(json!({ "id": publishable_key_id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn publishable_key_summary(
    item: chaos_core::contracts::PublishableKeyListItem,
) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "sales_channel_id": item.sales_channel_id.as_uuid(),
        "name": item.name,
        "public_key": item.public_key,
        "created_at": format_time(item.created_at),
        "revoked_at": item.revoked_at.map(format_time),
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
