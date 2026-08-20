use chaos_application::{
    ports::{StoreListItem, StoreMembershipItem},
    store::CreateStoreInput,
};
use chaos_domain::{
    identity::UserId,
    store::{StoreId, StoreRole},
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

use super::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateStoreParams {
    pub code: String,
    pub name: String,
    #[serde(default = "default_region")]
    pub default_region: String,
    #[serde(default = "default_currency")]
    pub default_currency: String,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListStoresParams {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListStoreMembersParams {}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AddStoreMemberParams {
    pub user_id: String,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct SetStoreMemberRoleParams {
    pub user_id: String,
    pub role: String,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct LeaveStoreParams {
    pub confirm: bool,
    pub idempotency_key: String,
}

#[tool_router(router = stores_tool_router, vis = "pub(super)")]
impl ChaosMcp {
    #[tool(
        description = "Create a Store owned by the authenticated User. This tool does not use \
                       X-Chaos-Store-Id because the Store does not exist yet. Requires confirm: \
                       true and an idempotency_key."
    )]
    async fn create_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateStoreParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let principal = match crate::auth::authenticate_principal(
            &self.state.access_key_authentication,
            &parts,
        )
        .await
        {
            Ok(principal) => principal,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        match self
            .state
            .create_store
            .execute(CreateStoreInput {
                user_id: principal.user_id,
                code: params.code,
                name: params.name,
                default_region: Some(params.default_region),
                default_currency: Some(params.default_currency),
                idempotency,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({ "id": output.store_id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List Stores currently available to the authenticated User. This tool \
                       does not require X-Chaos-Store-Id."
    )]
    async fn list_stores(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListStoresParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let principal = match crate::auth::authenticate_principal(
            &self.state.access_key_authentication,
            &parts,
        )
        .await
        {
            Ok(principal) => principal,
            Err(result) => return Ok(result),
        };
        let after = match params.cursor.as_deref().map(uuid::Uuid::parse_str) {
            Some(Ok(id)) => Some(StoreId::from_uuid(id)),
            Some(Err(_)) => {
                return Ok(CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": "cursor must be a valid Store UUID",
                })));
            }
            None => None,
        };
        match self
            .state
            .store_queries
            .list_stores(principal.user_id, after, params.limit.unwrap_or(20))
            .await
        {
            Ok(page) => {
                let items = page.items.into_iter().map(store_json).collect::<Vec<_>>();
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

    #[tool(description = "List Users who belong to the selected Store.")]
    async fn list_store_members(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_params): Parameters<ListStoreMembersParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        match self
            .state
            .store_membership_management
            .list(actor, store_id)
            .await
        {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(membership_json).collect::<Vec<_>>()
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Add a User as a Member of the selected Store. Owner role is required.")]
    async fn add_store_member(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AddStoreMemberParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let user_id = match uuid::Uuid::parse_str(&params.user_id) {
            Ok(id) => UserId::from_uuid(id),
            Err(_) => return Ok(invalid_uuid("user_id")),
        };
        let store_id = actor.store_id();
        let request = idempotency_request(params.idempotency_key.clone(), &params);
        match self
            .state
            .store_membership_management
            .add_member(actor, store_id, user_id, request)
            .await
        {
            Ok(item) => Ok(text_result(membership_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Change a selected Store member role. Owner role is required.")]
    async fn set_store_member_role(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<SetStoreMemberRoleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let user_id = match uuid::Uuid::parse_str(&params.user_id) {
            Ok(id) => UserId::from_uuid(id),
            Err(_) => return Ok(invalid_uuid("user_id")),
        };
        let role = match StoreRole::parse(&params.role) {
            Some(role) => role,
            None => {
                return Ok(CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": "role must be owner or member",
                })));
            }
        };
        let store_id = actor.store_id();
        let request = idempotency_request(params.idempotency_key.clone(), &params);
        match self
            .state
            .store_membership_management
            .set_role(actor, store_id, user_id, role, request)
            .await
        {
            Ok(item) => Ok(text_result(membership_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Leave the selected Store. The last Owner cannot leave.")]
    async fn leave_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<LeaveStoreParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.store_actor(&parts).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let store_id = actor.store_id();
        let request = idempotency_request(params.idempotency_key.clone(), &params);
        match self
            .state
            .store_membership_management
            .leave(actor, store_id, request)
            .await
        {
            Ok(()) => Ok(text_result(json!({ "left": true }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn store_json(item: StoreListItem) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "code": item.code.as_str(),
        "name": item.name,
        "default_region": item.default_region.as_str(),
        "default_currency": item.default_currency.as_str(),
        "status": item.status.as_str(),
        "role": item.role.as_str(),
    })
}

fn membership_json(item: StoreMembershipItem) -> serde_json::Value {
    json!({
        "user_id": item.user_id.as_uuid(),
        "role": item.role.as_str(),
        "created_at": item.created_at.to_string(),
        "updated_at": item.updated_at.to_string(),
    })
}

fn invalid_uuid(field: &'static str) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": "invalid_params",
        "message": format!("{field} must be a valid UUID"),
    }))
}

fn default_region() -> String {
    "US".into()
}

fn default_currency() -> String {
    "USD".into()
}
