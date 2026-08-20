use chaos_application::store::CreatePublishableKeyInput;
use chaos_domain::store::PublishableKeyId;
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::tools::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreatePublishableKeyParams {
    pub name: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListPublishableKeysParams {
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of Publishable Keys to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RevokePublishableKeyParams {
    /// The Publishable Key's UUID.
    pub publishable_key_id: String,
    /// Must be explicitly set to true. This action is irreversible and affects live
    /// store data — any caller presenting this key is immediately locked out.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = publishable_keys_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Create a new Publishable Key in the selected Store. The returned \
                        secret is shown exactly once and cannot be retrieved again — store it \
                        immediately. Requires confirm: true and an idempotency_key."
    )]
    async fn create_publishable_key(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreatePublishableKeyParams>,
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
            .publishable_key_management
            .create(CreatePublishableKeyInput {
                actor,
                store_id,
                name: params.name,
                idempotency,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "id": output.publishable_key.id().as_uuid(),
                "name": output.publishable_key.name(),
                "key_identifier": output.key_identifier,
                "display_suffix": output.display_suffix,
                "secret": output.plaintext.expose_secret(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List Publishable Keys in the selected Store. Never includes \
                        secret material. Paginated; use the returned next_cursor for more \
                        pages."
    )]
    async fn list_publishable_keys(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListPublishableKeysParams>,
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
                        idempotency_key."
    )]
    async fn revoke_publishable_key(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RevokePublishableKeyParams>,
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
        let publishable_key_id =
            match parse_uuid_field(&params.publishable_key_id, "publishable_key_id") {
                Ok(id) => PublishableKeyId::from_uuid(id),
                Err(result) => return Ok(result),
            };
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .publishable_key_management
            .revoke(actor, store_id, publishable_key_id, idempotency)
            .await
        {
            Ok(()) => Ok(text_result(json!({ "id": publishable_key_id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn publishable_key_summary(
    item: chaos_application::ports::PublishableKeyListItem,
) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "name": item.name,
        "key_identifier": item.key_identifier,
        "display_suffix": item.display_suffix,
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
