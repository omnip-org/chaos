use chaos_application::catalog::{
    ChangeProductStatusInput, CreateProductInput, CreateProductOptionInput,
    CreateProductSelectedOptionInput, CreateProductVariantInput, ProductPublicationInput,
    UpdateProductInput, UpdateProductVariantInput,
};
use chaos_domain::{
    catalog::{ProductId, ProductVariantId},
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

use crate::tools::ChaosMcp;
use crate::{
    error::{text_result, tool_error},
    mutation::{idempotency_request, require_confirmation},
};

#[derive(Deserialize, JsonSchema)]
pub struct ListProductsParams {
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of products to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetProductParams {
    /// The product's UUID.
    pub product_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductOptionParams {
    pub name: String,
    /// Every value this option can take (e.g. ["Blue", "Black"] for a "Color" option).
    pub values: Vec<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductSelectedOptionParams {
    /// Must match an option name declared in this product's `options`.
    pub option: String,
    /// Must match one of that option's declared `values`.
    pub value: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductVariantParams {
    /// Canonical variant title.
    pub title: String,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default = "default_true")]
    pub requires_shipping: bool,
    #[serde(default = "default_true")]
    pub track_inventory: bool,
    /// Exactly one value per declared product option, if any options are declared.
    #[serde(default)]
    pub selected_options: Vec<ProductSelectedOptionParams>,
    /// Arbitrary JSON (up to 32KB) for automation bookkeeping, e.g. an AI agent's own
    /// tracking fields. Not shown to shoppers.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateProductParams {
    /// URL-safe handle, unique within the Store (lowercase letters, digits, hyphens).
    pub handle: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Variant-selectable dimensions (e.g. Color, Size). Leave empty for a product with a
    /// single default variant and no options.
    #[serde(default)]
    pub options: Vec<ProductOptionParams>,
    /// At least one variant is required to later activate the product.
    pub variants: Vec<ProductVariantParams>,
    /// Arbitrary JSON (up to 32KB) for automation bookkeeping, e.g. an AI agent's own
    /// tracking fields. Not shown to shoppers.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeProductStatusParams {
    /// The product's UUID.
    pub product_id: String,
    /// Must be explicitly set to true. This action is irreversible via this tool
    /// and affects live store data. Review the product with get_product before confirming.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt. Reusing the same key
    /// with the same arguments replays the original result instead of repeating
    /// the mutation; reusing it with different arguments is a conflict.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateProductParams {
    /// The product's UUID.
    pub product_id: String,
    /// URL-safe handle, unique within the Store.
    pub handle: String,
    /// Canonical product title. This does not update variant titles, option names or values.
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Arbitrary JSON (up to 32KB) for automation bookkeeping, e.g. an AI agent's own
    /// tracking fields. Not shown to shoppers. This replaces the product's entire
    /// metadata, like every other field on this call; omit it (or pass null) to clear
    /// existing metadata rather than to preserve it.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt (see change-status tools).
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateProductVariantParams {
    /// The product's UUID.
    pub product_id: String,
    /// The product variant's UUID.
    pub product_variant_id: String,
    /// Canonical variant title.
    pub title: String,
    /// Set to null to remove the SKU.
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default = "default_true")]
    pub requires_shipping: bool,
    #[serde(default = "default_true")]
    pub track_inventory: bool,
    /// Arbitrary JSON (up to 32KB) for automation bookkeeping. Omitting this field clears
    /// existing metadata because the mutable canonical fields are replaced wholesale.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductPublicationParams {
    /// The product's UUID.
    pub product_id: String,
    /// The sales channel's UUID to publish to or unpublish from.
    pub sales_channel_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt (see change-status tools).
    pub idempotency_key: String,
}

#[tool_router(router = products_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List products in the selected Store, including draft and \
                        archived products. The returned title is the canonical Store catalog \
                        title from commerce.products, not locale-resolved storefront content. \
                        Paginated; use the returned next_cursor for more pages."
    )]
    async fn list_products(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListProductsParams>,
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
        let after = match params.cursor.as_deref().map(parse_product_cursor) {
            Some(Ok(id)) => Some(id),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let limit = params.limit.unwrap_or(20);

        match self
            .state
            .catalog_queries
            .list_products(actor, store_id, after, limit)
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
                            "title_source": "canonical",
                            "status": item.status.as_str(),
                            "variant_count": item.variant_count,
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
        description = "Get full details for a single product in the selected Store, including \
                        options, variants, their selected option values, and \
                        metadata (both product-level and per-variant). The product title and \
                        description are canonical fields from commerce.products, and each \
                        variant title is the canonical field from commerce.product_variants. \
                        This tool returns the Store's English catalog fields."
    )]
    async fn get_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetProductParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };

        match self
            .state
            .catalog_queries
            .get_product(actor, store_id, product_id)
            .await
        {
            Ok(detail) => Ok(text_result(json!({
                "id": detail.id.as_uuid(),
                "handle": detail.handle,
                "title": detail.title,
                "title_source": "canonical",
                "description": detail.description,
                "status": detail.status.as_str(),
                "options": detail.options.into_iter().map(|option| json!({
                    "id": option.id.as_uuid(),
                    "name": option.name,
                    "position": option.position,
                    "values": option.values.into_iter().map(|value| json!({
                        "id": value.id.as_uuid(),
                        "value": value.value,
                        "position": value.position,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "variants": detail.variants.into_iter().map(|variant| json!({
                    "id": variant.id.as_uuid(),
                    "title": variant.title,
                    "title_source": "canonical",
                    "sku": variant.sku,
                    "status": variant.status.as_str(),
                    "requires_shipping": variant.requires_shipping,
                    "track_inventory": variant.track_inventory,
                    "selected_options": variant.selected_options.into_iter().map(|selection| json!({
                        "option_id": selection.option_id.as_uuid(),
                        "option_name": selection.option_name,
                        "option_value_id": selection.option_value_id.as_uuid(),
                        "value": selection.value,
                    })).collect::<Vec<_>>(),
                    "metadata": variant.metadata,
                })).collect::<Vec<_>>(),
                "metadata": detail.metadata,
                "created_at": format_time(detail.created_at),
                "updated_at": format_time(detail.updated_at),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a new draft product in the selected Store, with its \
                        options, variants, and optional metadata (product-level and \
                        per-variant, arbitrary JSON up to 32KB, useful for automation \
                        bookkeeping). Product and variant titles are English catalog fields. The \
                        product starts as draft and is not visible anywhere until \
                        activate_product and publish_product are also called. Requires confirm: \
                        true and an idempotency_key."
    )]
    async fn create_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateProductParams>,
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

        let options = params
            .options
            .into_iter()
            .map(|option| CreateProductOptionInput {
                name: option.name,
                values: option.values,
            })
            .collect();
        let variants = params
            .variants
            .into_iter()
            .map(|variant| CreateProductVariantInput {
                title: variant.title,
                sku: variant.sku,
                requires_shipping: variant.requires_shipping,
                track_inventory: variant.track_inventory,
                selected_options: variant
                    .selected_options
                    .into_iter()
                    .map(|selection| CreateProductSelectedOptionInput {
                        option: selection.option,
                        value: selection.value,
                    })
                    .collect(),
                metadata: variant.metadata,
            })
            .collect();

        match self
            .state
            .create_product
            .execute(CreateProductInput {
                actor,
                store_id,
                handle: params.handle,
                title: params.title,
                description: params.description,
                options,
                variants,
                metadata: params.metadata,
                idempotency,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({ "id": output.product_id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update a product's handle, title, description, and metadata in the \
                        selected Store. These are canonical product fields only: this does not \
                        update variant titles, option names or values. \
                        Every field is replaced wholesale, including metadata (omit it to clear \
                        existing metadata). Requires confirm: true and an idempotency_key."
    )]
    async fn update_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateProductParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        match self
            .state
            .catalog_management
            .update(UpdateProductInput {
                actor,
                store_id,
                product_id,
                handle: params.handle,
                title: params.title,
                description: params.description,
                metadata: params.metadata,
                idempotency,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update one variant's canonical title, SKU, shipping flag, inventory \
                        tracking flag, and metadata in the selected Store. This updates the \
                        canonical catalog fields; it does not change selected option values. \
                        Mutable fields are replaced wholesale, and omitting metadata clears it. \
                        Requires confirm: true and an idempotency_key."
    )]
    async fn update_product_variant(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateProductVariantParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
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
            .catalog_management
            .update_variant(UpdateProductVariantInput {
                actor,
                store_id,
                product_id,
                product_variant_id,
                title: params.title,
                sku: params.sku,
                requires_shipping: params.requires_shipping,
                track_inventory: params.track_inventory,
                metadata: params.metadata,
                idempotency,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({
                "product_id": product_id.as_uuid(),
                "product_variant_id": id.as_uuid(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Activate a draft product in the selected Store, making it \
                        eligible for publication. Requires at least one variant. Requires \
                        confirm: true and an idempotency_key."
    )]
    async fn activate_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeProductStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_product_status(parts, params, true).await
    }

    #[tool(
        description = "Archive a product in the selected Store, removing it from \
                        sale without deleting it. Requires confirm: true and an idempotency_key."
    )]
    async fn archive_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeProductStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_product_status(parts, params, false).await
    }

    #[tool(
        description = "Publish an active product to a sales channel in the selected Store, making it visible on that channel. Requires confirm: true and \
                        an idempotency_key."
    )]
    async fn publish_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ProductPublicationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_product_publication(parts, params, true).await
    }

    #[tool(
        description = "Unpublish a product from a sales channel in the selected Store. Requires \
                        confirm: true and an idempotency_key."
    )]
    async fn unpublish_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ProductPublicationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_product_publication(parts, params, false).await
    }
}

impl ChaosMcp {
    async fn change_product_status(
        &self,
        parts: http::request::Parts,
        params: ChangeProductStatusParams,
        activate: bool,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        let input = ChangeProductStatusInput {
            actor,
            store_id,
            product_id,
            idempotency,
        };
        let result = if activate {
            self.state.catalog_management.activate(input).await
        } else {
            self.state.catalog_management.archive(input).await
        };
        match result {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    async fn change_product_publication(
        &self,
        parts: http::request::Parts,
        params: ProductPublicationParams,
        publish: bool,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let sales_channel_id = match parse_uuid_field(&params.sales_channel_id, "sales_channel_id")
        {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);

        let input = ProductPublicationInput {
            actor,
            store_id,
            product_id,
            sales_channel_id,
            idempotency,
        };
        let result = if publish {
            self.state.catalog_management.publish(input).await
        } else {
            self.state.catalog_management.unpublish(input).await
        };
        match result {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn parse_product_cursor(value: &str) -> Result<ProductId, CallToolResult> {
    parse_uuid_field(value, "cursor").map(ProductId::from_uuid)
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
