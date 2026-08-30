use chaos_core::catalog::{
    ChangeCollectionStatusInput, CollectionPublicationInput, CreateCollectionInput,
    ReplaceCollectionProductsInput, UpdateCollectionInput,
};
use chaos_domain::{
    catalog::{CollectionId, ProductId},
    store::SalesChannelId,
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

use crate::mcp::tools::ChaosMcp;
use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
};

#[derive(Deserialize, JsonSchema)]
pub struct ListCollectionsParams {
    /// The Store UUID to inspect.
    pub store_id: String,
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of collections to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetCollectionParams {
    /// The Store UUID containing the collection.
    pub store_id: String,
    /// The collection's UUID.
    pub collection_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateCollectionParams {
    /// The Store UUID to modify.
    pub store_id: String,
    /// URL-safe handle, unique within the Store.
    pub handle: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Optional JSON object (up to 32KB) for automation bookkeeping. Nested arrays and values
    /// are allowed, but the root must be an object. Not shown to shoppers.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateCollectionParams {
    /// The Store UUID containing the collection.
    pub store_id: String,
    /// The collection's UUID.
    pub collection_id: String,
    pub handle: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Optional JSON object (up to 32KB). Nested arrays and values are allowed, but the root
    /// must be an object. Replaces the collection's metadata; omit or pass null to clear it.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeCollectionStatusParams {
    /// The Store UUID containing the collection.
    pub store_id: String,
    /// The collection's UUID.
    pub collection_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AddProductsToCollectionParams {
    /// The Store UUID containing the collection.
    pub store_id: String,
    /// The collection's UUID.
    pub collection_id: String,
    /// The complete replacement set of product UUIDs for this collection, in display order.
    pub product_ids: Vec<String>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CollectionPublicationParams {
    /// The Store UUID containing the collection.
    pub store_id: String,
    /// The collection's UUID.
    pub collection_id: String,
    /// The sales channel's UUID to publish to or unpublish from.
    pub channel_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = collections_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List collections in the selected Store, including draft \
                        and archived collections. Paginated; use the returned next_cursor for \
                        more pages."
    )]
    async fn list_collections(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListCollectionsParams>,
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
        let after = match params.cursor.as_deref().map(parse_collection_cursor) {
            Some(Ok(id)) => Some(id),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let limit = params.limit.unwrap_or(20);

        match self
            .state
            .collection_administration
            .list(actor, store_id, after, limit)
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(|item| {
                        json!({
                            "id": item.id.as_uuid(),
                            "handle": item.handle,
                            "title": item.title,
                            "status": item.status.as_str(),
                            "product_count": item.product_count,
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
        description = "Get full details for a single collection in the selected Store, \
                        including its member products and published sales channels."
    )]
    async fn get_collection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetCollectionParams>,
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
        let collection_id = match parse_uuid_field(&params.collection_id, "collection_id") {
            Ok(id) => CollectionId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .collection_administration
            .get(actor, store_id, collection_id)
            .await
        {
            Ok(detail) => Ok(text_result(json!({
                "id": detail.id.as_uuid(),
                "handle": detail.handle,
                "title": detail.title,
                "description": detail.description,
                "status": detail.status.as_str(),
                "products": detail.products.into_iter().map(|item| json!({
                    "product_id": item.product_id.as_uuid(),
                    "position": item.position,
                })).collect::<Vec<_>>(),
                "published_channel_ids": detail.published_channel_ids.into_iter()
                    .map(|id| id.as_uuid()).collect::<Vec<_>>(),
                "metadata": detail.metadata,
                "created_at": format_time(detail.created_at),
                "updated_at": format_time(detail.updated_at),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a draft collection in the selected Store. Optional metadata must \
                        be a JSON object at the root; nested arrays are allowed. Requires confirm: true."
    )]
    async fn create_collection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateCollectionParams>,
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
            .collection_administration
            .create(CreateCollectionInput {
                actor,
                store_id,
                handle: params.handle,
                title: params.title,
                description: params.description,
                metadata: params.metadata,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update a collection's handle, title, description, and metadata in the \
                        selected Store. Metadata, when provided, must be a JSON object at the root; \
                        nested arrays are allowed. Metadata is replaced wholesale; omit it or pass \
                        null to clear it. Requires confirm: true."
    )]
    async fn update_collection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateCollectionParams>,
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
        let collection_id = match parse_uuid_field(&params.collection_id, "collection_id") {
            Ok(id) => CollectionId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        match self
            .state
            .collection_administration
            .update(UpdateCollectionInput {
                actor,
                store_id,
                collection_id,
                handle: params.handle,
                title: params.title,
                description: params.description,
                metadata: params.metadata,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Activate a draft collection in the selected Store. Requires confirm: true."
    )]
    async fn activate_collection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeCollectionStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_collection_status(parts, params, true).await
    }

    #[tool(description = "Archive a collection in the selected Store. Requires confirm: true.")]
    async fn archive_collection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeCollectionStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_collection_status(parts, params, false).await
    }

    #[tool(
        description = "Replace the full set of member products in a collection, in the Store \
                        selected by store_id. Pass the complete desired product_ids list, in \
                        display order — this replaces membership, it does not append. Requires \
                        confirm: true."
    )]
    async fn add_products_to_collection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AddProductsToCollectionParams>,
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
        let collection_id = match parse_uuid_field(&params.collection_id, "collection_id") {
            Ok(id) => CollectionId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let mut product_ids = Vec::with_capacity(params.product_ids.len());
        for value in &params.product_ids {
            match parse_uuid_field(value, "product_ids") {
                Ok(id) => product_ids.push(ProductId::from_uuid(id)),
                Err(result) => return Ok(result),
            }
        }
        match self
            .state
            .collection_administration
            .replace_products(ReplaceCollectionProductsInput {
                actor,
                store_id,
                collection_id,
                product_ids,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Publish an active collection to a sales channel in the selected Store. Requires confirm: true."
    )]
    async fn publish_collection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CollectionPublicationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_collection_publication(parts, params, true)
            .await
    }

    #[tool(
        description = "Unpublish a collection from a sales channel in the selected Store. Requires confirm: true."
    )]
    async fn unpublish_collection(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CollectionPublicationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_collection_publication(parts, params, false)
            .await
    }
}

impl ChaosMcp {
    async fn change_collection_status(
        &self,
        parts: http::request::Parts,
        params: ChangeCollectionStatusParams,
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
        let collection_id = match parse_uuid_field(&params.collection_id, "collection_id") {
            Ok(id) => CollectionId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let input = ChangeCollectionStatusInput {
            actor,
            store_id,
            collection_id,
            now: self.state.clock.now(),
        };
        let result = if activate {
            self.state.collection_administration.activate(input).await
        } else {
            self.state.collection_administration.archive(input).await
        };
        match result {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    async fn change_collection_publication(
        &self,
        parts: http::request::Parts,
        params: CollectionPublicationParams,
        publish: bool,
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
        let collection_id = match parse_uuid_field(&params.collection_id, "collection_id") {
            Ok(id) => CollectionId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let channel_id = match parse_uuid_field(&params.channel_id, "channel_id") {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let input = CollectionPublicationInput {
            actor,
            store_id,
            collection_id,
            channel_id,
            now: self.state.clock.now(),
        };
        let result = if publish {
            self.state.collection_administration.publish(input).await
        } else {
            self.state.collection_administration.unpublish(input).await
        };
        match result {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn parse_collection_cursor(value: &str) -> Result<CollectionId, CallToolResult> {
    parse_uuid_field(value, "cursor").map(CollectionId::from_uuid)
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
