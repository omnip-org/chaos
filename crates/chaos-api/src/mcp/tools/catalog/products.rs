use chaos_core::catalog::{
    ChangeProductStatusInput, CreateProductInput, CreateProductOptionInput,
    CreateProductSelectedOptionInput, CreateProductVariantInput, PatchProductInput,
    PatchProductVariantInput, ProductConfigurationDraft, ProductConfigurationOptionInput,
    ProductConfigurationOptionValueInput, ProductConfigurationVariantInput,
    ProductPublicationInput, SyncProductConfigurationInput, UpdateProductInput,
    UpdateProductVariantInput, validate_product_configuration,
};
use chaos_domain::{
    catalog::{ProductId, ProductStatus, ProductVariantId},
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
pub struct ListProductsParams {
    /// The Store UUID to inspect.
    pub store_id: String,
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of products to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
    /// Optional full-text search over handle, title, description, variant title, and SKU.
    #[serde(default)]
    pub query: Option<String>,
    /// Optional lifecycle filter: draft, active, or archived.
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetProductParams {
    /// The Store UUID containing the product.
    pub store_id: String,
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
    pub track_inventory: bool,
    /// Exactly one value per declared product option, if any options are declared.
    #[serde(default)]
    pub selected_options: Vec<ProductSelectedOptionParams>,
    /// Optional JSON object (up to 32KB) for automation bookkeeping, e.g. an AI agent's own
    /// tracking fields. Nested arrays and values are allowed, but the root must be an object.
    /// Not shown to shoppers.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductConfigurationOptionValueParams {
    /// Stable Option Value UUID from the workspace. Omit to create a new value.
    #[serde(default)]
    pub id: Option<String>,
    pub value: String,
    /// Display order within the option. Defaults to the array index.
    #[serde(default)]
    pub position: Option<u16>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductConfigurationOptionParams {
    /// Stable Option UUID from the workspace. Omit to create a new option.
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    /// Display order among product options. Defaults to the array index.
    #[serde(default)]
    pub position: Option<u16>,
    pub values: Vec<ProductConfigurationOptionValueParams>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductConfigurationSelectedOptionParams {
    /// Existing Option UUID. Use this with option_value_id, or with value.
    #[serde(default)]
    pub option_id: Option<String>,
    /// Existing Option Value UUID. Preferred when updating an existing product.
    #[serde(default)]
    pub option_value_id: Option<String>,
    /// Option name, useful when creating a complete new configuration.
    #[serde(default)]
    pub option: Option<String>,
    /// Option value, useful when creating a complete new configuration.
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductConfigurationVariantParams {
    /// Stable Variant UUID from the workspace. Omit to create a new variant.
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default = "default_true")]
    pub track_inventory: bool,
    /// Exactly one selection per active option. Existing configurations should use IDs;
    /// name/value references are also supported for a complete new configuration.
    pub selected_options: Vec<ProductConfigurationSelectedOptionParams>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductConfigurationParams {
    pub store_id: String,
    pub product_id: String,
    /// Complete desired active option/value state. Omitted existing options and values are archived.
    pub options: Vec<ProductConfigurationOptionParams>,
    /// Complete desired active variant state. Omitted existing variants are archived.
    pub variants: Vec<ProductConfigurationVariantParams>,
    /// Reject the write if the product has changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true for synchronization.
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PreviewProductConfigurationParams {
    pub store_id: String,
    pub product_id: String,
    pub options: Vec<ProductConfigurationOptionParams>,
    pub variants: Vec<ProductConfigurationVariantParams>,
    /// Optional revision to compare with the current workspace.
    #[serde(default)]
    pub expected_revision: Option<i64>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateProductParams {
    /// The Store UUID to modify.
    pub store_id: String,
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
    /// Optional JSON object (up to 32KB) for automation bookkeeping, e.g. an AI agent's own
    /// tracking fields. Nested arrays and values are allowed, but the root must be an object.
    /// Not shown to shoppers.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ChangeProductStatusParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This action affects live store data. Review the product
    /// with get_product before confirming.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateProductParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// URL-safe handle, unique within the Store.
    pub handle: String,
    /// Canonical product title. This does not update variant titles, option names or values.
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Optional JSON object (up to 32KB) for automation bookkeeping, e.g. an AI agent's own
    /// tracking fields. Nested arrays and values are allowed, but the root must be an object.
    /// Not shown to shoppers. This replaces the product's entire metadata, like every other
    /// field on this call; omit it (or pass null) to clear existing metadata rather than to
    /// preserve it. Managed Product metadata media references must be preserved exactly; use
    /// the Media tools to change them.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Reject the write if the product has changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UpdateProductVariantParams {
    /// The Store UUID containing the product.
    pub store_id: String,
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
    pub track_inventory: bool,
    /// Optional JSON object (up to 32KB) for automation bookkeeping. Nested arrays and values
    /// are allowed, but the root must be an object. Omitting this field or passing null clears
    /// existing metadata because the mutable canonical fields are replaced wholesale.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Reject the write if the product has changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PatchProductParams {
    pub store_id: String,
    pub product_id: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Omitted preserves metadata; null clears it; an object replaces it.
    #[serde(default)]
    pub metadata: Option<Option<serde_json::Value>>,
    #[serde(default)]
    pub expected_revision: Option<i64>,
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PatchProductVariantParams {
    pub store_id: String,
    pub product_id: String,
    pub product_variant_id: String,
    #[serde(default)]
    pub title: Option<String>,
    /// Omitted preserves the SKU; null clears it; a string replaces it.
    #[serde(default)]
    pub sku: Option<Option<String>>,
    #[serde(default)]
    pub track_inventory: Option<bool>,
    /// Omitted preserves metadata; null clears it; an object replaces it.
    #[serde(default)]
    pub metadata: Option<Option<serde_json::Value>>,
    #[serde(default)]
    pub expected_revision: Option<i64>,
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductPublicationParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// The sales channel's UUID to publish to or unpublish from.
    pub sales_channel_id: String,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = products_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List products in the selected Store, including draft and \
                        archived products. The returned title is the canonical Store catalog \
                        title from commerce.products, not translated storefront content. \
                        Paginated; use the returned next_cursor for more pages."
    )]
    async fn list_products(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListProductsParams>,
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
        let after = match params.cursor.as_deref().map(parse_product_cursor) {
            Some(Ok(id)) => Some(id),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let limit = params.limit.unwrap_or(20);
        let status = match params.status.as_deref() {
            Some(value) => match ProductStatus::parse(value) {
                Some(status) => Some(status),
                None => {
                    return Ok(CallToolResult::structured_error(json!({
                        "code": "invalid_params",
                        "message": "status must be one of draft, active, or archived",
                    })));
                }
            },
            None => None,
        };

        match self
            .state
            .catalog_queries
            .list_products(
                actor,
                store_id,
                after,
                limit,
                params.query.as_deref(),
                status,
            )
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
                            "revision": item.revision,
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
                        metadata (both product-level and per-variant; each metadata value must be \
                        a JSON object at the root, though nested arrays are allowed). The product title and \
                        description are canonical fields from commerce.products, and each \
                        variant title is the canonical field from commerce.product_variants. \
                        This tool returns the Store's English catalog fields."
    )]
    async fn get_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetProductParams>,
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
                "revision": detail.revision,
                "options": detail.options.into_iter().map(|option| json!({
                    "id": option.id.as_uuid(),
                    "name": option.name,
                    "position": option.position,
                    "archived_at": option.archived_at.map(format_time),
                    "values": option.values.into_iter().map(|value| json!({
                        "id": value.id.as_uuid(),
                        "value": value.value,
                        "position": value.position,
                        "archived_at": value.archived_at.map(format_time),
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "variants": detail.variants.into_iter().map(|variant| json!({
                    "id": variant.id.as_uuid(),
                    "title": variant.title,
                    "title_source": "canonical",
                    "sku": variant.sku,
                    "status": variant.status.as_str(),
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
        description = "Get an editable Product workspace snapshot in one call. It includes the \
                        Product revision, all options and values including archived records, all \
                        Variants and selected value IDs, every raw Product/Option Value/Variant \
                        media rule including archived links, and published sales channel IDs. \
                        Use this snapshot as the source of truth before configuration or media \
                        batch updates."
    )]
    async fn get_product_workspace(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetProductParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        match self
            .state
            .product_workspace_queries
            .get(actor, store_id, product_id)
            .await
        {
            Ok(workspace) => Ok(text_result(json!({
                "product": product_detail_json(workspace.product),
                "media": workspace.media.into_iter().map(workspace_media_json).collect::<Vec<_>>(),
                "published_sales_channel_ids": workspace.publications.into_iter().map(|publication| publication.sales_channel_id.as_uuid()).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Create a new draft product in the selected Store, with its \
                        options, variants, and optional metadata (product-level and \
                        per-variant; each metadata value must be a JSON object at the root, \
                        though nested arrays are allowed, up to 32KB, useful for automation \
                        bookkeeping). Product and variant titles are English catalog fields. The \
                        product starts as draft and is not visible anywhere until \
                        activate_product and publish_product are also called. Requires confirm: \
                        true."
    )]
    async fn create_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateProductParams>,
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
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "id": output.product_id.as_uuid(),
                "options": output.options.into_iter().map(|option| json!({
                    "id": option.id.as_uuid(),
                    "values": option.values.into_iter().map(|value| json!({
                        "id": value.id.as_uuid(),
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "variants": output.variants.into_iter().map(|variant| json!({
                    "id": variant.id.as_uuid(),
                    "selected_options": variant.selected_options.into_iter().map(|(option_id, value_id)| json!({
                        "option_id": option_id.as_uuid(),
                        "option_value_id": value_id.as_uuid(),
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Preview a complete Product option/value/Variant configuration before writing it. \
                        Existing records should include their stable IDs; omitted existing records \
                        will be archived by sync_product_configuration. Selections may use IDs or \
                        option/value names. Returns validation errors, warnings, affected IDs, and \
                        the current Product revision. This is read-only."
    )]
    async fn preview_product_configuration(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<PreviewProductConfigurationParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let draft = match build_configuration_draft(&params.options, &params.variants) {
            Ok(draft) => draft,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        match self
            .state
            .product_workspace_queries
            .get(actor, store_id, product_id)
            .await
        {
            Ok(workspace) => {
                let validation = validate_product_configuration(&workspace.product, &draft);
                let mut result = configuration_validation_json(
                    &validation,
                    workspace.product.revision,
                    params.expected_revision,
                );
                result["normalized_draft"] = configuration_draft_json(&draft);
                Ok(text_result(result))
            }
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Atomically synchronize a Product's complete active options, option values, \
                        and Variants. Use get_product_workspace first and preserve IDs for records \
                        that should remain; omit IDs only for new records. Omitted existing options, \
                        values, and Variants are archived, while supplied Variants are active. The \
                        same stable Option Value IDs may be selected by many Variants. Use \
                        expected_revision to protect against concurrent edits. An active Product \
                        must retain at least one Variant. Requires confirm: true."
    )]
    async fn sync_product_configuration(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ProductConfigurationParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let draft = match build_configuration_draft(&params.options, &params.variants) {
            Ok(draft) => draft,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .product_configuration
            .sync(SyncProductConfigurationInput {
                actor,
                store_id,
                product_id,
                draft,
                expected_revision: params.expected_revision,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "product_id": output.product_id.as_uuid(),
                "revision": output.revision,
                "options": output.draft.options.into_iter().map(|option| json!({
                    "id": option.id.as_uuid(),
                    "name": option.name,
                    "position": option.position,
                    "values": option.values.into_iter().map(|value| json!({
                        "id": value.id.as_uuid(),
                        "value": value.value,
                        "position": value.position,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "variants": output.draft.variants.into_iter().map(|variant| json!({
                    "id": variant.id.as_uuid(),
                    "title": variant.title,
                    "sku": variant.sku,
                    "track_inventory": variant.track_inventory,
                    "selected_option_value_ids": variant.selected_option_value_ids.into_iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
                    "metadata": variant.metadata,
                })).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Update a product's handle, title, description, and metadata in the \
                        selected Store. These are canonical product fields only: this does not \
                        update variant titles, option names or values. \
                        Metadata, when provided, must be a JSON object at the root; nested arrays \
                        are allowed. Every field is replaced wholesale, including metadata (omit \
                        it or pass null to clear existing metadata). Managed Product metadata media references must be \
                        preserved exactly; use the Media tools to change them. Requires confirm: true."
    )]
    async fn update_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateProductParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
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
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "id": output.product_id.as_uuid(),
                "revision": output.revision,
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Update one variant's canonical title, SKU, inventory \
                        tracking flag, and metadata in the selected Store. Metadata, when provided, \
                        must be a JSON object at the root; nested arrays are allowed. This updates the \
                        canonical catalog fields; it does not change selected option values. \
                        Mutable fields are replaced wholesale, and omitting metadata or passing null clears it. \
                        Requires confirm: true.")]
    async fn update_product_variant(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UpdateProductVariantParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let product_variant_id =
            match parse_uuid_field(&params.product_variant_id, "product_variant_id") {
                Ok(id) => ProductVariantId::from_uuid(id),
                Err(result) => return Ok(result),
            };
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
                track_inventory: params.track_inventory,
                metadata: params.metadata,
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "product_id": product_id.as_uuid(),
                "product_variant_id": product_variant_id.as_uuid(),
                "revision": output.revision,
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Patch selected canonical Product fields without replacing omitted fields. \
                        Omitted values are preserved; metadata is tri-state (omit to preserve, null \
                        to clear, object to replace). Use expected_revision to avoid overwriting a \
                        newer workspace. Requires confirm: true."
    )]
    async fn patch_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<PatchProductParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        match self
            .state
            .catalog_management
            .patch(PatchProductInput {
                store_id: actor.store_id(),
                actor,
                product_id,
                handle: params.handle,
                title: params.title,
                description: params.description,
                metadata: params.metadata,
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "product_id": output.product_id.as_uuid(),
                "revision": output.revision,
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Patch selected canonical fields on one Product Variant. Omitted fields are \
                        preserved; SKU and metadata are tri-state (omit to preserve, null to clear, \
                        value to replace). This does not change selected option values. Use \
                        expected_revision to avoid overwriting a newer workspace. Requires confirm: true."
    )]
    async fn patch_product_variant(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<PatchProductVariantParams>,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let product_variant_id =
            match parse_uuid_field(&params.product_variant_id, "product_variant_id") {
                Ok(id) => ProductVariantId::from_uuid(id),
                Err(result) => return Ok(result),
            };
        match self
            .state
            .catalog_management
            .patch_variant(PatchProductVariantInput {
                store_id: actor.store_id(),
                actor,
                product_id,
                product_variant_id,
                title: params.title,
                sku: params.sku,
                track_inventory: params.track_inventory,
                metadata: params.metadata,
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "product_id": output.product_id.as_uuid(),
                "product_variant_id": product_variant_id.as_uuid(),
                "revision": output.revision,
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Activate a draft product in the selected Store, making it \
                        eligible for publication. Requires at least one variant. Requires \
                        expected_revision when editing from a workspace, and confirm: true."
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
                        sale without deleting it. Use expected_revision when editing from a \
                        workspace. Requires confirm: true."
    )]
    async fn archive_product(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ChangeProductStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_product_status(parts, params, false).await
    }

    #[tool(
        description = "Publish an active product to a sales channel in the selected Store, making it visible on that channel. Use expected_revision when editing from a workspace. Requires confirm: true."
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
                        expected_revision when editing from a workspace, and confirm: true."
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let input = ChangeProductStatusInput {
            actor,
            store_id,
            product_id,
            expected_revision: params.expected_revision,
        };
        let result = if activate {
            self.state.catalog_management.activate(input).await
        } else {
            self.state.catalog_management.archive(input).await
        };
        match result {
            Ok(output) => Ok(text_result(json!({
                "id": output.product_id.as_uuid(),
                "revision": output.revision,
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    async fn change_product_publication(
        &self,
        parts: http::request::Parts,
        params: ProductPublicationParams,
        publish: bool,
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
        let product_id = match parse_uuid_field(&params.product_id, "product_id") {
            Ok(id) => ProductId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let sales_channel_id = match parse_uuid_field(&params.sales_channel_id, "sales_channel_id")
        {
            Ok(id) => SalesChannelId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let input = ProductPublicationInput {
            actor,
            store_id,
            product_id,
            sales_channel_id,
            expected_revision: params.expected_revision,
        };
        let result = if publish {
            self.state.catalog_management.publish(input).await
        } else {
            self.state.catalog_management.unpublish(input).await
        };
        match result {
            Ok(output) => Ok(text_result(json!({
                "id": output.product_id.as_uuid(),
                "revision": output.revision,
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn parse_product_cursor(value: &str) -> Result<ProductId, CallToolResult> {
    parse_uuid_field(value, "cursor").map(ProductId::from_uuid)
}

fn build_configuration_draft(
    options: &[ProductConfigurationOptionParams],
    variants: &[ProductConfigurationVariantParams],
) -> Result<ProductConfigurationDraft, CallToolResult> {
    let options = options
        .iter()
        .enumerate()
        .map(|(option_index, option)| {
            let id = option
                .id
                .as_deref()
                .map(|value| {
                    parse_uuid_field(value, "option.id")
                        .map(chaos_domain::catalog::ProductOptionId::from_uuid)
                })
                .transpose()?;
            let values = option
                .values
                .iter()
                .enumerate()
                .map(|(value_index, value)| {
                    let id = value
                        .id
                        .as_deref()
                        .map(|value| {
                            parse_uuid_field(value, "option.values.id")
                                .map(chaos_domain::catalog::ProductOptionValueId::from_uuid)
                        })
                        .transpose()?
                        .unwrap_or_default();
                    Ok(ProductConfigurationOptionValueInput {
                        id,
                        value: value.value.clone(),
                        position: value.position.unwrap_or(value_index as u16),
                    })
                })
                .collect::<Result<Vec<_>, CallToolResult>>()?;
            Ok(ProductConfigurationOptionInput {
                id: id.unwrap_or_default(),
                name: option.name.clone(),
                position: option.position.unwrap_or(option_index as u16),
                values,
            })
        })
        .collect::<Result<Vec<_>, CallToolResult>>()?;

    let variants = variants
        .iter()
        .map(|variant| {
            let id = variant
                .id
                .as_deref()
                .map(|value| parse_uuid_field(value, "variant.id").map(chaos_domain::catalog::ProductVariantId::from_uuid))
                .transpose()?
                .unwrap_or_default();
            let selected_option_value_ids = variant
                .selected_options
                .iter()
                .map(|selection| {
                    if let Some(option_id) = selection.option_id.as_deref() {
                        let option_id = parse_uuid_field(option_id, "selected_options.option_id")
                            .map(chaos_domain::catalog::ProductOptionId::from_uuid)?;
                        let option = options
                            .iter()
                            .find(|option| option.id == option_id)
                            .ok_or_else(|| {
                                invalid_parameter(
                                    "selected_options",
                                    "option_id must identify an option in this draft",
                                )
                            })?;
                        if let Some(value_id) = selection.option_value_id.as_deref() {
                            let value_id = parse_uuid_field(
                                value_id,
                                "selected_options.option_value_id",
                            )
                            .map(chaos_domain::catalog::ProductOptionValueId::from_uuid)?;
                            if !option.values.iter().any(|value| value.id == value_id) {
                                return Err(invalid_parameter(
                                    "selected_options",
                                    "option_id and option_value_id do not describe a value in that option",
                                ));
                            }
                            return Ok(value_id);
                        }
                        let value = selection.value.as_deref().ok_or_else(|| {
                            invalid_parameter(
                                "selected_options",
                                "an option_id selection needs option_value_id or value",
                            )
                        })?;
                        return option
                            .values
                            .iter()
                            .find(|candidate| candidate.value.eq_ignore_ascii_case(value))
                            .map(|candidate| candidate.id)
                            .ok_or_else(|| {
                                invalid_parameter(
                                    "selected_options",
                                    "the selection names an unknown option value",
                                )
                            });
                    }
                    if let Some(value_id) = selection.option_value_id.as_deref() {
                        return parse_uuid_field(value_id, "selected_options.option_value_id")
                            .map(chaos_domain::catalog::ProductOptionValueId::from_uuid);
                    }
                    let option_name = selection.option.as_deref().ok_or_else(|| {
                        invalid_parameter(
                            "selected_options",
                            "each selection needs option_value_id or option plus value",
                        )
                    })?;
                    let value = selection.value.as_deref().ok_or_else(|| {
                        invalid_parameter(
                            "selected_options",
                            "each name-based selection needs a value",
                        )
                    })?;
                    let option = options
                        .iter()
                        .find(|option| option.name.eq_ignore_ascii_case(option_name))
                        .ok_or_else(|| {
                            invalid_parameter(
                                "selected_options",
                                "the selection names an unknown option",
                            )
                        })?;
                    if let Some(option_id) = selection.option_id.as_deref() {
                        let option_id = parse_uuid_field(option_id, "selected_options.option_id")
                            .map(chaos_domain::catalog::ProductOptionId::from_uuid)?;
                        if option.id != option_id {
                            return Err(invalid_parameter(
                                "selected_options",
                                "option does not match option_id",
                            ));
                        }
                    }
                    option
                        .values
                        .iter()
                        .find(|candidate| candidate.value.eq_ignore_ascii_case(value))
                        .map(|candidate| candidate.id)
                        .ok_or_else(|| {
                            invalid_parameter(
                                "selected_options",
                                "the selection names an unknown option value",
                            )
                        })
                })
                .collect::<Result<Vec<_>, CallToolResult>>()?;
            Ok(ProductConfigurationVariantInput {
                id,
                title: variant.title.clone(),
                sku: variant.sku.clone(),
                track_inventory: variant.track_inventory,
                selected_option_value_ids,
                metadata: variant.metadata.clone(),
            })
        })
        .collect::<Result<Vec<_>, CallToolResult>>()?;

    Ok(ProductConfigurationDraft { options, variants })
}

fn configuration_draft_json(draft: &ProductConfigurationDraft) -> serde_json::Value {
    json!({
        "options": draft.options.iter().map(|option| json!({
            "id": option.id.as_uuid(),
            "name": option.name,
            "position": option.position,
            "values": option.values.iter().map(|value| json!({
                "id": value.id.as_uuid(),
                "value": value.value,
                "position": value.position,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "variants": draft.variants.iter().map(|variant| json!({
            "id": variant.id.as_uuid(),
            "title": variant.title,
            "sku": variant.sku,
            "track_inventory": variant.track_inventory,
            "selected_option_value_ids": variant.selected_option_value_ids.iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
            "metadata": variant.metadata,
        })).collect::<Vec<_>>(),
    })
}

fn product_detail_json(detail: chaos_core::contracts::CatalogProductDetail) -> serde_json::Value {
    json!({
        "id": detail.id.as_uuid(),
        "handle": detail.handle,
        "title": detail.title,
        "title_source": "canonical",
        "description": detail.description,
        "status": detail.status.as_str(),
        "revision": detail.revision,
        "options": detail.options.into_iter().map(|option| json!({
            "id": option.id.as_uuid(),
            "name": option.name,
            "position": option.position,
            "archived_at": option.archived_at.map(format_time),
            "values": option.values.into_iter().map(|value| json!({
                "id": value.id.as_uuid(),
                "value": value.value,
                "position": value.position,
                "archived_at": value.archived_at.map(format_time),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "variants": detail.variants.into_iter().map(|variant| json!({
            "id": variant.id.as_uuid(),
            "title": variant.title,
            "title_source": "canonical",
            "sku": variant.sku,
            "status": variant.status.as_str(),
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
    })
}

fn workspace_media_json(item: chaos_core::contracts::ProductMediaAssetItem) -> serde_json::Value {
    let chaos_core::contracts::ProductMediaAssetItem {
        asset,
        product_id,
        scope,
        alt_text,
        position,
        archived_at,
    } = item;
    let mut value = json!({
        "id": asset.id.as_uuid(),
        "store_id": asset.store_id.as_uuid(),
        "file_name": asset.file_name,
        "media_type": asset.media_type,
        "kind": asset.kind.as_str(),
        "byte_size": asset.byte_size,
        "sha256_hex": asset.sha256_hex,
        "status": asset.status.as_str(),
        "public_url": asset.public_url,
        "created_at": format_time(asset.created_at),
        "updated_at": format_time(asset.updated_at),
        "product_id": product_id.as_uuid(),
        "alt_text": alt_text,
        "position": position,
        "archived_at": archived_at.map(format_time),
    });
    match scope {
        chaos_core::contracts::ProductMediaScope::Product => value["scope"] = json!("product"),
        chaos_core::contracts::ProductMediaScope::OptionValue {
            option_id,
            option_value_id,
        } => {
            value["scope"] = json!("option_value");
            value["option_id"] = json!(option_id.as_uuid());
            value["option_value_id"] = json!(option_value_id.as_uuid());
        }
        chaos_core::contracts::ProductMediaScope::Variant { product_variant_id } => {
            value["scope"] = json!("variant");
            value["product_variant_id"] = json!(product_variant_id.as_uuid());
        }
    }
    value
}

fn configuration_validation_json(
    validation: &chaos_core::catalog::ProductConfigurationValidation,
    current_revision: i64,
    expected_revision: Option<i64>,
) -> serde_json::Value {
    let revision_matches = expected_revision.is_none_or(|expected| expected == current_revision);
    let mut errors = validation
        .errors
        .iter()
        .map(|violation| json!({ "field": violation.field, "reason": violation.reason }))
        .collect::<Vec<_>>();
    if !revision_matches {
        errors.push(json!({
            "field": "expected_revision",
            "reason": "the Product changed; refresh the workspace before applying this draft",
        }));
    }
    json!({
        "valid": errors.is_empty(),
        "current_revision": current_revision,
        "expected_revision": expected_revision,
        "revision_matches": revision_matches,
        "errors": errors,
        "warnings": validation.warnings.iter().map(|violation| json!({
            "field": violation.field,
            "reason": violation.reason,
        })).collect::<Vec<_>>(),
        "changes": {
            "options_added": validation.options_added.iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
            "options_archived": validation.options_archived.iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
            "values_added": validation.values_added.iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
            "values_archived": validation.values_archived.iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
            "variants_added": validation.variants_added.iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
            "variants_archived": validation.variants_archived.iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
        },
    })
}

fn invalid_parameter(field: &'static str, message: &'static str) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": "invalid_params",
        "message": format!("{field}: {message}"),
    }))
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
