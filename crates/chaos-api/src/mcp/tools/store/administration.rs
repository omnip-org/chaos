use chaos_core::{
    contracts::{SalesChannelAdminItem, ShippingCountryAdminItem, StoreAdminItem},
    store::{
        ChangeSalesChannelStatusInput, ChangeStoreStatusInput, CreateSalesChannelInput,
        SetShippingCountryInput, UpdateSalesChannelInput, UpdateStoreInput,
    },
};
use chaos_domain::store::SalesChannelId;
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

use crate::mcp::tools::{ChaosMcp, StoreIdParams};
use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateStoreParams {
    /// The Store UUID to modify.
    pub store_id: String,
    pub name: String,
    /// Two-letter ISO 3166-1 region code.
    pub region: String,
    #[serde(default)]
    pub meta: Option<serde_json::Value>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeStoreStatusParams {
    /// The Store UUID to modify.
    pub store_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetSalesChannelParams {
    /// The Store UUID containing the sales channel.
    pub store_id: String,
    /// The sales channel's UUID.
    pub channel_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateSalesChannelParams {
    /// The Store UUID to modify.
    pub store_id: String,
    pub name: String,
    /// Absolute HTTP(S) origin used by customer-facing links for this channel.
    pub origin: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateSalesChannelParams {
    /// The Store UUID containing the sales channel.
    pub store_id: String,
    /// The sales channel's UUID.
    pub channel_id: String,
    pub name: String,
    /// Absolute HTTP(S) origin used by customer-facing links for this channel.
    pub origin: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeSalesChannelStatusParams {
    /// The Store UUID containing the sales channel.
    pub store_id: String,
    /// The sales channel's UUID.
    pub channel_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct SetShippingCountryParams {
    /// The Store UUID to modify.
    pub store_id: String,
    /// Two-letter ISO 3166-1 alpha-2 destination country code.
    pub country_code: String,
    /// Whether the Store accepts shipments to this country.
    pub enabled: bool,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = store_admin_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(description = "Get the selected Store.")]
    async fn get_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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

    #[tool(description = "List shipping destination countries in the selected Store.")]
    async fn list_shipping_countries(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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
        match self
            .state
            .store_administration
            .list_shipping_countries(actor, store_id)
            .await
        {
            Ok(items) => Ok(text_result(json!({
                "items": items
                    .into_iter()
                    .map(shipping_country_json)
                    .collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Enable or disable one shipping destination country in the selected Store. \
                        Requires confirm: true."
    )]
    async fn set_shipping_country(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<SetShippingCountryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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
        match self
            .state
            .store_administration
            .set_shipping_country(SetShippingCountryInput {
                actor,
                store_id,
                country_code: params.country_code,
                enabled: params.enabled,
            })
            .await
        {
            Ok(item) => Ok(text_result(shipping_country_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update the selected Store's name, region, and metadata. \
                        Store currency is fixed at creation. Requires confirm: true."
    )]
    async fn update_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateStoreParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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
        match self
            .state
            .store_administration
            .update_store(UpdateStoreInput {
                actor,
                store_id,
                name: params.name,
                region: params.region,
                meta: params.meta,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Reactivate the selected Store, making it live. Requires \
                        confirm: true."
    )]
    async fn activate_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeStoreStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_store_status(parts, params, true).await
    }

    #[tool(
        description = "Archive the selected Store. Requires confirm: true and an \
                        confirm: true."
    )]
    async fn archive_store(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeStoreStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_store_status(parts, params, false).await
    }

    #[tool(description = "List sales channels in the selected Store, including \
                        archived ones.")]
    async fn list_sales_channels(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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

    #[tool(description = "Get a single sales channel's details in the selected Store.")]
    async fn get_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetSalesChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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
        let channel_id = match parse_uuid_field(&params.channel_id, "channel_id") {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .store_administration
            .get_sales_channel(actor, store_id, channel_id)
            .await
        {
            Ok(item) => Ok(text_result(sales_channel_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a sales channel in the selected Store, including its \
                        storefront origin. Requires \
                        confirm: true."
    )]
    async fn create_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateSalesChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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
        match self
            .state
            .store_administration
            .create_sales_channel(CreateSalesChannelInput {
                actor,
                store_id,
                name: params.name,
                origin: params.origin,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update a sales channel's name and storefront origin in the \
                        selected Store. \
                        Requires confirm: true."
    )]
    async fn update_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateSalesChannelParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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
        let channel_id = match parse_uuid_field(&params.channel_id, "channel_id") {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        match self
            .state
            .store_administration
            .update_sales_channel(UpdateSalesChannelInput {
                actor,
                store_id,
                channel_id,
                name: params.name,
                origin: params.origin,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Activate a sales channel in the selected Store. Requires \
                        confirm: true."
    )]
    async fn activate_sales_channel(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeSalesChannelStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_sales_channel_status(parts, params, true).await
    }

    #[tool(
        description = "Archive a sales channel in the selected Store. The last active \
                        channel cannot be archived. Requires confirm: true and an \
                        confirm: true."
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
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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
        let input = ChangeStoreStatusInput { actor, store_id };
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
        let actor = match crate::mcp::auth::authenticate_mcp(
            &self.state.mcp_oauth,
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
        let channel_id = match parse_uuid_field(&params.channel_id, "channel_id") {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let input = ChangeSalesChannelStatusInput {
            actor,
            store_id,
            channel_id,
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
        "name": item.name,
        "region": item.region.as_str(),
        "currency": item.currency.as_str(),
        "meta": item.meta,
        "status": item.status.as_str(),
        "created_at": format_time(item.created_at),
        "updated_at": format_time(item.updated_at),
    })
}

fn sales_channel_json(item: SalesChannelAdminItem) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "name": item.name,
        "origin": item.origin.as_str(),
        "status": item.status.as_str(),
        "created_at": format_time(item.created_at),
        "updated_at": format_time(item.updated_at),
    })
}

fn shipping_country_json(item: ShippingCountryAdminItem) -> serde_json::Value {
    json!({
        "country_code": item.country_code,
        "enabled": item.enabled,
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
