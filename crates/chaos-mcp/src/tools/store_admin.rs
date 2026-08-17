use chaos_application::{
    merchant::{
        ChangeSalesChannelStatusInput, ChangeStoreStatusInput, CreateSalesChannelInput,
        UpdateSalesChannelInput, UpdateStoreInput,
    },
    ports::{AdminActor, SalesChannelAdminItem, StoreAdminItem},
};
use chaos_domain::merchant::{ApiKeyScope, SalesChannelId};
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

use super::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateStoreParams {
    /// URL-safe code, unique within the merchant account.
    pub code: String,
    pub name: String,
    /// Two-letter ISO 3166-1 region code.
    pub default_region: String,
    /// Three-letter ISO 4217 currency code.
    pub default_currency: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeStoreStatusParams {
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetSalesChannelParams {
    /// The sales channel's UUID.
    pub sales_channel_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateSalesChannelParams {
    /// URL-safe code, unique within the Store.
    pub code: String,
    pub name: String,
    /// "web", "mobile", "point_of_sale", "marketplace", or "custom".
    pub kind: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateSalesChannelParams {
    /// The sales channel's UUID.
    pub sales_channel_id: String,
    pub code: String,
    pub name: String,
    /// "web", "mobile", "point_of_sale", "marketplace", or "custom".
    pub kind: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeSalesChannelStatusParams {
    /// The sales channel's UUID.
    pub sales_channel_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = store_admin_tool_router, vis = "pub(super)")]
impl ChaosMcp {
    #[tool(description = "Get the Store bound to this API key.")]
    async fn get_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminRead,
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

        match self
            .state
            .store_administration
            .get_store(actor, store_id)
            .await
        {
            Ok(item) => Ok(text_result(store_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update the Store bound to this API key's code, name, default region, \
                        and default currency. Requires confirm: true and an idempotency_key."
    )]
    async fn update_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateStoreParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .store_administration
            .update_store(UpdateStoreInput {
                actor,
                store_id,
                code: params.code,
                name: params.name,
                default_region: params.default_region,
                default_currency: params.default_currency,
                idempotency,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Activate the Store bound to this API key, making it live. Requires \
                        confirm: true and an idempotency_key."
    )]
    async fn activate_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeStoreStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_store_status(parts, params, true).await
    }

    #[tool(
        description = "Archive the Store bound to this API key. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn archive_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeStoreStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_store_status(parts, params, false).await
    }

    #[tool(
        description = "List sales channels in the Store bound to this API key, including \
                        archived ones."
    )]
    async fn list_sales_channels(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminRead,
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

        match self
            .state
            .store_administration
            .list_sales_channels(actor, store_id, None, 100)
            .await
        {
            Ok(page) => Ok(text_result(json!({
                "items": page.items.into_iter().map(sales_channel_json).collect::<Vec<_>>(),
                "has_more": page.has_more,
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get a single sales channel's details in the Store bound to this API key."
    )]
    async fn get_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetSalesChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminRead,
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
        let sales_channel_id = match parse_uuid_field(&params.sales_channel_id, "sales_channel_id")
        {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .store_administration
            .get_sales_channel(actor, store_id, sales_channel_id)
            .await
        {
            Ok(item) => Ok(text_result(sales_channel_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a sales channel in the Store bound to this API key. Requires \
                        confirm: true and an idempotency_key."
    )]
    async fn create_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateSalesChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .store_administration
            .create_sales_channel(CreateSalesChannelInput {
                actor,
                store_id,
                code: params.code,
                name: params.name,
                kind: params.kind,
                idempotency,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update a sales channel's code, name, and kind in the Store bound to \
                        this API key. Requires confirm: true and an idempotency_key."
    )]
    async fn update_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateSalesChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let sales_channel_id = match parse_uuid_field(&params.sales_channel_id, "sales_channel_id")
        {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .store_administration
            .update_sales_channel(UpdateSalesChannelInput {
                actor,
                store_id,
                sales_channel_id,
                code: params.code,
                name: params.name,
                kind: params.kind,
                idempotency,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Activate a sales channel in the Store bound to this API key. Requires \
                        confirm: true and an idempotency_key."
    )]
    async fn activate_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeSalesChannelStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_sales_channel_status(parts, params, true).await
    }

    #[tool(
        description = "Archive a sales channel in the Store bound to this API key. The default \
                        channel cannot be archived. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn archive_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeSalesChannelStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_sales_channel_status(parts, params, false).await
    }
}

impl ChaosMcp {
    async fn change_store_status(
        &self,
        parts: http::request::Parts,
        params: ChangeStoreStatusParams,
        activate: bool,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        let input = ChangeStoreStatusInput {
            actor,
            store_id,
            idempotency,
        };
        let result = if activate {
            self.state.store_administration.activate_store(input).await
        } else {
            self.state.store_administration.archive_store(input).await
        };
        match result {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    async fn change_sales_channel_status(
        &self,
        parts: http::request::Parts,
        params: ChangeSalesChannelStatusParams,
        activate: bool,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::auth::authenticate_machine(
            &self.state.api_key_authentication,
            &parts,
            ApiKeyScope::StoreAdminWrite,
        )
        .await
        {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let AdminActor::Machine(machine) = &actor else {
            unreachable!("authenticate_machine always returns AdminActor::Machine")
        };
        let store_id = machine.store_id;
        let sales_channel_id = match parse_uuid_field(&params.sales_channel_id, "sales_channel_id")
        {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        let input = ChangeSalesChannelStatusInput {
            actor,
            store_id,
            sales_channel_id,
            idempotency,
        };
        let result = if activate {
            self.state
                .store_administration
                .activate_sales_channel(input)
                .await
        } else {
            self.state
                .store_administration
                .archive_sales_channel(input)
                .await
        };
        match result {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn store_json(item: StoreAdminItem) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "code": item.code.as_str(),
        "name": item.name,
        "default_region": item.default_region.as_str(),
        "default_currency": item.default_currency.as_str(),
        "status": item.status.as_str(),
        "created_at": format_time(item.created_at),
        "updated_at": format_time(item.updated_at),
    })
}

fn sales_channel_json(item: SalesChannelAdminItem) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "code": item.code.as_str(),
        "name": item.name,
        "kind": item.kind.as_str(),
        "status": item.status.as_str(),
        "is_default": item.is_default,
        "created_at": format_time(item.created_at),
        "updated_at": format_time(item.updated_at),
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
