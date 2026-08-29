use chaos_core::{
    catalog::{
        ArchiveMediaAssetInput, ArchiveProductMediaInput, ArchiveProductMetaMediaInput,
        ArchiveProductOptionValueMediaInput, ArchiveProductVariantMediaInput,
        ArchiveReviewMediaInput, AttachProductMediaInput, AttachProductMetaMediaInput,
        AttachProductOptionValueMediaInput, AttachProductVariantMediaInput, AttachReviewMediaInput,
        BatchReplaceProductMediaInput, BatchReplaceProductMediaTarget, CompleteMediaUploadInput,
        CreateMediaUploadInput, ListMediaAssetsInput, ProductMediaItemInput, ProductMediaTarget,
        RefreshMediaUploadInput, ReplaceProductMediaInput, ReplaceProductOptionValueMediaInput,
        ReplaceProductVariantMediaInput, RestoreMediaAssetInput, resolve_product_media,
    },
    contracts::{
        MediaAssetItem, MediaUploadRequest, ProductMediaAssetItem, ProductMetaMediaAssetItem,
        ReviewMediaAssetItem,
    },
};
use chaos_domain::catalog::{
    MediaAssetId, MediaAssetStatus, MediaKind, ProductId, ProductOptionId, ProductOptionValueId,
    ProductVariantId, ReviewId,
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

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PrepareMediaUploadParams {
    /// The Store UUID that owns the Media Asset.
    pub store_id: String,
    /// Exact byte size of the file that the Host will upload.
    pub byte_size: u64,
    /// Lowercase SHA-256 hex digest of the exact file bytes.
    pub sha256_hex: String,
    /// Original file name, e.g. "hero.webp".
    pub file_name: String,
    /// MIME type, e.g. "image/webp", "image/jpeg", or "video/mp4".
    pub media_type: String,
    /// Must be explicitly set to true. This creates a pending reusable Media Asset.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RefreshMediaUploadParams {
    /// The Store UUID that owns the Media Asset.
    pub store_id: String,
    /// The pending Media Asset's UUID.
    pub media_asset_id: String,
    /// Must be explicitly set to true. This issues a new short-lived upload request.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CompleteMediaUploadParams {
    /// The Store UUID that owns the Media Asset.
    pub store_id: String,
    /// The Media Asset's UUID.
    pub media_asset_id: String,
    /// Must be explicitly set to true. This verifies the object and marks it ready.
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetMediaAssetParams {
    /// The Store UUID that owns the Media Asset.
    pub store_id: String,
    /// The Media Asset's UUID.
    pub media_asset_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ArchiveMediaAssetParams {
    /// The Store UUID that owns the Media Asset.
    pub store_id: String,
    /// The Media Asset's UUID.
    pub media_asset_id: String,
    /// Must be explicitly set to true. The asset must have no active attachments.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RestoreMediaAssetParams {
    /// The Store UUID that owns the Media Asset.
    pub store_id: String,
    /// An archived ready Media Asset's UUID.
    pub media_asset_id: String,
    /// Must be explicitly set to true.
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListMediaAssetsParams {
    /// The Store UUID that owns the Media Assets.
    pub store_id: String,
    /// Opaque cursor from a previous page's next_cursor.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Optional lifecycle status: pending_upload, ready, or archived.
    #[serde(default)]
    pub status: Option<String>,
    /// Optional media kind: image or video.
    #[serde(default)]
    pub kind: Option<String>,
    /// Optional exact lowercase SHA-256 digest.
    #[serde(default)]
    pub sha256_hex: Option<String>,
    /// Optional case-insensitive file-name fragment.
    #[serde(default)]
    pub file_name: Option<String>,
    /// Maximum number of assets to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ResolveProductMediaParams {
    pub store_id: String,
    pub product_id: String,
    pub product_variant_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct BatchProductMediaTargetParams {
    /// Target scope: product, option_value, or variant.
    pub scope: String,
    #[serde(default)]
    pub option_id: Option<String>,
    #[serde(default)]
    pub option_value_id: Option<String>,
    #[serde(default)]
    pub product_variant_id: Option<String>,
    /// Complete desired gallery for this target. An empty array clears it.
    pub items: Vec<ProductMediaItemParams>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct BatchReplaceProductMediaParams {
    pub store_id: String,
    pub product_id: String,
    /// Up to 100 distinct targets, applied atomically in one transaction.
    pub targets: Vec<BatchProductMediaTargetParams>,
    #[serde(default)]
    pub expected_revision: Option<i64>,
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AttachProductMediaParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// A ready reusable Media Asset's UUID. This is the fallback gallery for
    /// every Variant without a more specific media rule.
    pub media_asset_id: String,
    /// Alt text for accessibility and SEO.
    #[serde(default)]
    pub alt_text: String,
    /// Display order among the product's media (0-99).
    #[serde(default)]
    pub position: u16,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This changes catalog presentation.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AttachProductOptionValueMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The Product Option's UUID, such as Color or Length.
    pub option_id: String,
    /// The Option Value's UUID, such as Red or 100cm.
    pub option_value_id: String,
    /// A ready reusable Media Asset's UUID. The same asset may be attached to
    /// multiple Option Values.
    pub media_asset_id: String,
    /// Alt text for accessibility and SEO.
    #[serde(default)]
    pub alt_text: String,
    /// Display order among this Option Value's media (0-99).
    #[serde(default)]
    pub position: u16,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This changes catalog presentation.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AttachProductVariantMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The exact Product Variant's UUID.
    pub product_variant_id: String,
    /// A ready reusable Media Asset's UUID.
    pub media_asset_id: String,
    /// Alt text for accessibility and SEO.
    #[serde(default)]
    pub alt_text: String,
    /// Display order among this Variant's media (0-99).
    #[serde(default)]
    pub position: u16,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This changes catalog presentation.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProductMediaItemParams {
    /// A ready reusable Media Asset's UUID.
    pub media_asset_id: String,
    /// Alt text for accessibility and SEO.
    #[serde(default)]
    pub alt_text: String,
    /// Display order within the target (0-99). Positions must be unique.
    #[serde(default)]
    pub position: u16,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ReplaceProductMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The complete desired Product gallery. An empty array clears the gallery.
    pub items: Vec<ProductMediaItemParams>,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This atomically replaces the gallery.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ReplaceProductOptionValueMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The Product Option's UUID.
    pub option_id: String,
    /// The Option Value's UUID.
    pub option_value_id: String,
    /// The complete desired Option Value gallery. An empty array clears it.
    pub items: Vec<ProductMediaItemParams>,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This atomically replaces the gallery.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ReplaceProductVariantMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The exact Product Variant's UUID.
    pub product_variant_id: String,
    /// The complete desired Variant gallery. An empty array clears it.
    pub items: Vec<ProductMediaItemParams>,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This atomically replaces the gallery.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AttachReviewMediaParams {
    /// The Store UUID containing the review.
    pub store_id: String,
    /// The pending top-level review's UUID.
    pub review_id: String,
    /// A ready image Media Asset's UUID.
    pub media_asset_id: String,
    /// Alt text for the published review image.
    #[serde(default)]
    pub alt_text: String,
    /// Display order among the review's images (0-99).
    #[serde(default)]
    pub position: u16,
    /// Must be explicitly set to true. This changes review content.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AttachProductMetaMediaParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// A ready image Media Asset's UUID.
    pub media_asset_id: String,
    /// RFC 6901 JSON Pointer into product metadata, e.g. "/landing_page/hero/image".
    pub meta_path: String,
    /// Alt text stored with the metadata reference.
    #[serde(default)]
    pub alt_text: String,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This updates product metadata.
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListProductMediaParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListProductOptionValueMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The Product Option's UUID.
    pub option_id: String,
    /// The Option Value's UUID.
    pub option_value_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListProductVariantMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The exact Product Variant's UUID.
    pub product_variant_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListReviewMediaParams {
    /// The Store UUID containing the review.
    pub store_id: String,
    /// The review's UUID.
    pub review_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListProductMetaMediaParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ArchiveProductMediaParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// The Media Asset's UUID.
    pub media_asset_id: String,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This removes the Product attachment.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ArchiveProductOptionValueMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The Product Option's UUID.
    pub option_id: String,
    /// The Option Value's UUID.
    pub option_value_id: String,
    /// The Media Asset's UUID.
    pub media_asset_id: String,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This removes the Option Value attachment.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ArchiveProductVariantMediaParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product's UUID.
    pub product_id: String,
    /// The exact Product Variant's UUID.
    pub product_variant_id: String,
    /// The Media Asset's UUID.
    pub media_asset_id: String,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This removes the Variant attachment.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ArchiveReviewMediaParams {
    /// The Store UUID containing the review.
    pub store_id: String,
    /// The review Media Asset's UUID.
    pub review_id: String,
    /// The Media Asset's UUID.
    pub media_asset_id: String,
    /// Must be explicitly set to true. This removes the Review attachment.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ArchiveProductMetaMediaParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// The Media Asset's UUID.
    pub media_asset_id: String,
    /// The same RFC 6901 JSON Pointer used when attaching the media.
    pub meta_path: String,
    /// Reject the write if the Product changed since this revision.
    #[serde(default)]
    pub expected_revision: Option<i64>,
    /// Must be explicitly set to true. This removes the metadata reference.
    pub confirm: bool,
}

#[tool_router(router = media_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "List reusable Media Assets in the selected Store for discovery before \
                        attaching or replacing Product media. Filter by status, kind, exact SHA-256, \
                        or file-name fragment. Paginated; use next_cursor for the next page."
    )]
    async fn list_media_assets(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListMediaAssetsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let status = match params.status.as_deref() {
            Some(value) => match MediaAssetStatus::parse(value) {
                Some(status) => Some(status),
                None => {
                    return Ok(invalid_media_parameter(
                        "status",
                        "unknown Media Asset status",
                    ));
                }
            },
            None => None,
        };
        let kind = match params.kind.as_deref() {
            Some(value) => match MediaKind::parse(value) {
                Some(kind) => Some(kind),
                None => return Ok(invalid_media_parameter("kind", "must be image or video")),
            },
            None => None,
        };
        let after = match params.cursor.as_deref() {
            Some(value) => match parse_media_asset_id(value) {
                Ok(id) => Some(id),
                Err(result) => return Ok(result),
            },
            None => None,
        };
        match self
            .state
            .media_administration
            .list_assets(ListMediaAssetsInput {
                store_id: actor.store_id(),
                actor,
                after,
                limit: params.limit.unwrap_or(20),
                status,
                kind,
                sha256_hex: params.sha256_hex,
                file_name: params.file_name,
            })
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(media_asset_json)
                    .collect::<Vec<_>>();
                let next_cursor = page.has_more.then(|| {
                    items
                        .last()
                        .and_then(|item| item["id"].as_str().map(String::from))
                });
                Ok(text_result(json!({
                    "items": items,
                    "has_more": page.has_more,
                    "next_cursor": next_cursor.flatten(),
                })))
            }
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Restore an archived reusable Media Asset whose original object URL is \
                        retained. This is useful after a link replacement automatically archived \
                        an otherwise unused asset; restore it before attaching it again. Requires \
                        confirm: true."
    )]
    async fn restore_media_asset(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RestoreMediaAssetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        match self
            .state
            .media_administration
            .restore(RestoreMediaAssetInput {
                actor,
                store_id,
                media_asset_id,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(asset) => Ok(text_result(media_asset_json(asset))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Preview the effective storefront gallery for one Product Variant. Resolution \
                        is exact Variant media first, then the union of matching Option Value media \
                        with duplicate assets removed, then Product fallback media. Only active ready \
                        assets are returned. This is read-only."
    )]
    async fn resolve_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ResolveProductMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let product_variant_id = match parse_product_variant_id(&params.product_variant_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let workspace = match self
            .state
            .product_workspace_queries
            .get(actor, store_id, product_id)
            .await
        {
            Ok(workspace) => workspace,
            Err(error) => return Ok(tool_error(error)),
        };
        match resolve_product_media(&workspace, product_variant_id) {
            Ok(resolved) => Ok(text_result(json!({
                "product_id": product_id.as_uuid(),
                "product_variant_id": product_variant_id.as_uuid(),
                "source": resolved.source.as_str(),
                "matched_option_value_ids": resolved.matched_option_value_ids.into_iter().map(|id| id.as_uuid()).collect::<Vec<_>>(),
                "items": resolved.items.into_iter().map(product_media_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Atomically replace the complete galleries of up to 100 distinct Product, \
                        Option Value, and Variant targets in one transaction. The same ready Media \
                        Asset may appear in any number of targets. Empty items clear a target. Use \
                        expected_revision to prevent overwriting another catalog edit. Requires \
                        confirm: true."
    )]
    async fn batch_replace_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<BatchReplaceProductMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let targets = match params
            .targets
            .iter()
            .map(parse_batch_media_target)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(targets) => targets,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .batch_replace_product(BatchReplaceProductMediaInput {
                store_id: actor.store_id(),
                actor,
                product_id,
                targets,
                expected_revision: params.expected_revision,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "product_id": product_id.as_uuid(),
                "revision": output.revision,
                "items": output.items.into_iter().map(product_media_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Prepare a reusable direct Media upload in the selected Store. Provide \
                        only file metadata and the exact lowercase SHA-256 digest; do not send \
                        file bytes. Returns a pending media_asset_id and a short-lived presigned \
                        PUT request in the tool result for the MCP Host. After the Host uploads the bytes, \
                        call complete_media_upload, then attach the ready asset to a Product, \
                        Review, or Product metadata path. Requires confirm: true."
    )]
    async fn prepare_media_upload(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<PrepareMediaUploadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let store_id = actor.store_id();
        match self
            .state
            .media_administration
            .create(CreateMediaUploadInput {
                actor,
                store_id,
                file_name: params.file_name,
                media_type: params.media_type,
                byte_size: params.byte_size,
                sha256_hex: params.sha256_hex,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(created) => Ok(prepared_media_result(
                media_asset_json(created.asset),
                created.upload,
                "The MCP Host must PUT the original file bytes using the upload request in the tool result, then call complete_media_upload. After completion, attach the ready asset to its business target.",
            )),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Refresh the short-lived direct-upload request for a pending reusable Media \
                        Asset. The MCP Host must upload the exact bytes declared during preparation. \
                        The refreshed upload request is returned in the tool result. Requires confirm: true."
    )]
    async fn refresh_media_upload(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RefreshMediaUploadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .refresh_upload(RefreshMediaUploadInput {
                store_id: actor.store_id(),
                media_asset_id,
                actor,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(upload) => Ok(refreshed_media_result(
                media_asset_id,
                upload,
                "The MCP Host must PUT the original file bytes using the upload request in the tool result, then call complete_media_upload.",
            )),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Complete a reusable direct Media upload. Call this only after the MCP Host \
                        has PUT the exact file bytes to the prepared upload request. Chaos verifies \
                        object existence, MIME type, byte size, and SHA-256 before marking the \
                        Media Asset ready. Requires confirm: true."
    )]
    async fn complete_media_upload(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CompleteMediaUploadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .complete_upload(CompleteMediaUploadInput {
                store_id: actor.store_id(),
                media_asset_id,
                actor,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(asset) => Ok(text_result(media_asset_json(asset))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Get one reusable Media Asset and its verification status in the selected \
                        Store."
    )]
    async fn get_media_asset(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetMediaAssetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .get(actor, store_id, media_asset_id)
            .await
        {
            Ok(asset) => Ok(text_result(media_asset_json(asset))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Archive a reusable Media Asset that has no active Product, Review, or \
                        Product metadata attachments. Requires confirm: true."
    )]
    async fn archive_media_asset(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ArchiveMediaAssetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .archive(ArchiveMediaAssetInput {
                store_id,
                media_asset_id,
                actor,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(asset) => Ok(text_result(media_asset_json(asset))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Attach or update a ready reusable Media Asset in the Product fallback \
                        gallery. Variants use this gallery when they have no exact Variant or \
                        matching Option Value media. The same physical asset may be reused by \
                        many targets. Requires confirm: true."
    )]
    async fn attach_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AttachProductMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .attach_product(AttachProductMediaInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                alt_text: params.alt_text,
                position: params.position,
                changed_at: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_mutation_json(
                output.item,
                output.revision,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Attach or update a ready reusable Media Asset for one Product Option Value, \
                        such as Color=Red or Length=100cm. The same physical asset may be reused \
                        by multiple Option Values. For a Variant without exact media, matching \
                        Option Value media takes precedence over Product media. Requires confirm: true."
    )]
    async fn attach_product_option_value_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AttachProductOptionValueMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let option_id = match parse_product_option_id(&params.option_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let option_value_id = match parse_product_option_value_id(&params.option_value_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .attach_product_option_value(AttachProductOptionValueMediaInput {
                actor,
                store_id,
                product_id,
                option_id,
                option_value_id,
                media_asset_id,
                alt_text: params.alt_text,
                position: params.position,
                changed_at: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_mutation_json(
                output.item,
                output.revision,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Attach or update a ready reusable Media Asset for one exact Product \
                        Variant. Exact Variant media overrides matching Option Value media and \
                        Product fallback media. Requires confirm: true."
    )]
    async fn attach_product_variant_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AttachProductVariantMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let product_variant_id = match parse_product_variant_id(&params.product_variant_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .attach_product_variant(AttachProductVariantMediaInput {
                actor,
                store_id,
                product_id,
                product_variant_id,
                media_asset_id,
                alt_text: params.alt_text,
                position: params.position,
                changed_at: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_mutation_json(
                output.item,
                output.revision,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Atomically replace the complete Product fallback gallery. Existing links \
                        missing from items are archived, supplied links are upserted, and an empty \
                        items array clears the gallery. Use this for bulk add/update/remove/reorder. \
                        Requires confirm: true."
    )]
    async fn replace_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ReplaceProductMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let items = match parse_product_media_items(params.items) {
            Ok(items) => items,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .replace_product(ReplaceProductMediaInput {
                actor,
                store_id,
                product_id,
                items,
                now: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_replacement_json(output))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Atomically replace the complete gallery for one Product Option Value. \
                        Existing links missing from items are archived, supplied links are upserted, \
                        and an empty items array clears the gallery. Requires confirm: true."
    )]
    async fn replace_product_option_value_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ReplaceProductOptionValueMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let option_id = match parse_product_option_id(&params.option_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let option_value_id = match parse_product_option_value_id(&params.option_value_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let items = match parse_product_media_items(params.items) {
            Ok(items) => items,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .replace_product_option_value(ReplaceProductOptionValueMediaInput {
                actor,
                store_id,
                product_id,
                option_id,
                option_value_id,
                items,
                now: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_replacement_json(output))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Atomically replace the complete gallery for one exact Product Variant. \
                        Existing links missing from items are archived, supplied links are upserted, \
                        and an empty items array clears the gallery. Requires confirm: true."
    )]
    async fn replace_product_variant_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ReplaceProductVariantMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let product_variant_id = match parse_product_variant_id(&params.product_variant_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let items = match parse_product_media_items(params.items) {
            Ok(items) => items,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .replace_product_variant(ReplaceProductVariantMediaInput {
                actor,
                store_id,
                product_id,
                product_variant_id,
                items,
                now: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_replacement_json(output))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Attach a ready image Media Asset to a pending top-level Review. The asset \
                        must be prepared and completed first. Requires confirm: true."
    )]
    async fn attach_review_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AttachReviewMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let review_id = match parse_review_id(&params.review_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .attach_review(AttachReviewMediaInput {
                actor,
                store_id,
                review_id,
                media_asset_id,
                alt_text: params.alt_text,
                position: params.position,
            })
            .await
        {
            Ok(item) => Ok(text_result(review_media_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Attach a ready image Media Asset to a Product metadata JSON Pointer. Chaos \
                        sets the managed media_asset_id and alt_text fields at the path while \
                        preserving other presentation fields, and records the attachment \
                        transactionally. Replacing an existing path archives its previous link. Use \
                        expected_revision to avoid overwriting a newer Product edit. Requires \
                        confirm: true."
    )]
    async fn attach_product_meta_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AttachProductMetaMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .attach_product_meta(AttachProductMetaMediaInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                meta_path: params.meta_path,
                alt_text: params.alt_text,
                now: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "revision": output.revision,
                "item": product_meta_media_json(output.item),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List every Product media rule in the selected Store, including Product \
                        fallback links, Option Value links, exact Variant links, pending assets, \
                        and archived links. Each item includes a scope so the complete inheritance \
                        graph can be inspected."
    )]
    async fn list_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListProductMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .list_product(actor, store_id, product_id)
            .await
        {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(product_media_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List all Media attachments for one active Product Option Value, including \
                        pending and archived links."
    )]
    async fn list_product_option_value_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListProductOptionValueMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let option_id = match parse_product_option_id(&params.option_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let option_value_id = match parse_product_option_value_id(&params.option_value_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .list_product_option_value(actor, store_id, product_id, option_id, option_value_id)
            .await
        {
            Ok(items) => Ok(text_result(product_media_list_json(items))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List all Media attachments for one active Product Variant, including \
                        pending and archived links."
    )]
    async fn list_product_variant_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListProductVariantMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let product_variant_id = match parse_product_variant_id(&params.product_variant_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .list_product_variant(actor, store_id, product_id, product_variant_id)
            .await
        {
            Ok(items) => Ok(text_result(product_media_list_json(items))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List all Review Media attachments in the selected Store, including \
                        pending and archived links."
    )]
    async fn list_review_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListReviewMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let review_id = match parse_review_id(&params.review_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .list_review(actor, store_id, review_id)
            .await
        {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(review_media_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List all Product metadata Media attachments in the selected Store, \
                        including archived links and their JSON Pointer paths."
    )]
    async fn list_product_meta_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListProductMetaMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .list_product_meta(actor, store_id, product_id)
            .await
        {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(product_meta_media_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Archive a Product gallery attachment. If the reusable Media Asset has no \
                        other active attachments, it is archived as well. Requires confirm: true."
    )]
    async fn archive_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ArchiveProductMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .archive_product(ArchiveProductMediaInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                now: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_mutation_json(
                output.item,
                output.revision,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Archive a Review Media attachment. If the reusable Media Asset has no \
                        other active attachments, it is archived as well. Requires confirm: true."
    )]
    async fn archive_review_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ArchiveReviewMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let review_id = match parse_review_id(&params.review_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .archive_review(ArchiveReviewMediaInput {
                actor,
                store_id,
                review_id,
                media_asset_id,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(item) => Ok(text_result(review_media_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Archive a Product Option Value media attachment. If the reusable Media \
                        Asset has no other active attachments, it is archived as well. Requires \
                        confirm: true."
    )]
    async fn archive_product_option_value_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ArchiveProductOptionValueMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let option_id = match parse_product_option_id(&params.option_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let option_value_id = match parse_product_option_value_id(&params.option_value_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .archive_product_option_value(ArchiveProductOptionValueMediaInput {
                actor,
                store_id,
                product_id,
                option_id,
                option_value_id,
                media_asset_id,
                now: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_mutation_json(
                output.item,
                output.revision,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Archive an exact Product Variant media attachment. If the reusable Media \
                        Asset has no other active attachments, it is archived as well. Requires \
                        confirm: true."
    )]
    async fn archive_product_variant_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ArchiveProductVariantMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let product_variant_id = match parse_product_variant_id(&params.product_variant_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .archive_product_variant(ArchiveProductVariantMediaInput {
                actor,
                store_id,
                product_id,
                product_variant_id,
                media_asset_id,
                now: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(product_media_mutation_json(
                output.item,
                output.revision,
            ))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Archive a Product metadata Media attachment and remove its metadata \
                        reference if it still points at the same asset. Use expected_revision to \
                        avoid overwriting a newer Product edit. Requires confirm: true."
    )]
    async fn archive_product_meta_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ArchiveProductMetaMediaParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = match self.authenticate(&parts, &params.store_id).await {
            Ok(actor) => actor,
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        if let Err(result) = require_confirmation(params.confirm) {
            return Ok(result);
        }
        let product_id = match parse_product_id(&params.product_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        let media_asset_id = match parse_media_asset_id(&params.media_asset_id) {
            Ok(id) => id,
            Err(result) => return Ok(result),
        };
        match self
            .state
            .media_administration
            .archive_product_meta(ArchiveProductMetaMediaInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                meta_path: params.meta_path,
                now: self.state.clock.now(),
                expected_revision: params.expected_revision,
            })
            .await
        {
            Ok(output) => Ok(text_result(json!({
                "revision": output.revision,
                "item": product_meta_media_json(output.item),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    async fn authenticate(
        &self,
        parts: &http::request::Parts,
        store_id: &str,
    ) -> Result<chaos_core::contracts::AdminActor, CallToolResult> {
        crate::mcp::auth::authenticate_mcp(
            &self.state.access_key_authentication,
            &self.state.store_queries,
            parts,
            store_id,
        )
        .await
    }
}

fn parse_batch_media_target(
    target: &BatchProductMediaTargetParams,
) -> Result<BatchReplaceProductMediaTarget, CallToolResult> {
    let items = target
        .items
        .iter()
        .map(parse_product_media_item)
        .collect::<Result<Vec<_>, _>>()?;
    let parsed = match target.scope.as_str() {
        "product" => {
            if target.option_id.is_some()
                || target.option_value_id.is_some()
                || target.product_variant_id.is_some()
            {
                return Err(invalid_media_parameter(
                    "targets",
                    "product targets must not include option or variant IDs",
                ));
            }
            ProductMediaTarget::Product
        }
        "option_value" => {
            let option_id = target
                .option_id
                .as_deref()
                .ok_or_else(|| {
                    invalid_media_parameter("targets", "option_value targets need option_id")
                })
                .and_then(parse_product_option_id)?;
            let option_value_id = target
                .option_value_id
                .as_deref()
                .ok_or_else(|| {
                    invalid_media_parameter("targets", "option_value targets need option_value_id")
                })
                .and_then(parse_product_option_value_id)?;
            if target.product_variant_id.is_some() {
                return Err(invalid_media_parameter(
                    "targets",
                    "option_value targets must not include product_variant_id",
                ));
            }
            ProductMediaTarget::OptionValue {
                option_id,
                option_value_id,
            }
        }
        "variant" => {
            let product_variant_id = target
                .product_variant_id
                .as_deref()
                .ok_or_else(|| {
                    invalid_media_parameter("targets", "variant targets need product_variant_id")
                })
                .and_then(parse_product_variant_id)?;
            if target.option_id.is_some() || target.option_value_id.is_some() {
                return Err(invalid_media_parameter(
                    "targets",
                    "variant targets must not include option IDs",
                ));
            }
            ProductMediaTarget::Variant { product_variant_id }
        }
        _ => {
            return Err(invalid_media_parameter(
                "targets.scope",
                "must be product, option_value, or variant",
            ));
        }
    };
    Ok(BatchReplaceProductMediaTarget {
        target: parsed,
        items,
    })
}

fn parse_product_media_item(
    item: &ProductMediaItemParams,
) -> Result<ProductMediaItemInput, CallToolResult> {
    Ok(ProductMediaItemInput {
        media_asset_id: parse_media_asset_id(&item.media_asset_id)?,
        alt_text: item.alt_text.clone(),
        position: item.position,
    })
}

fn invalid_media_parameter(field: &'static str, message: &'static str) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": "invalid_params",
        "message": format!("{field}: {message}"),
    }))
}

fn media_asset_json(item: MediaAssetItem) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "store_id": item.store_id.as_uuid(),
        "file_name": item.file_name,
        "media_type": item.media_type,
        "kind": item.kind.as_str(),
        "byte_size": item.byte_size,
        "sha256_hex": item.sha256_hex,
        "status": item.status.as_str(),
        "public_url": item.public_url,
        "created_at": format_time(item.created_at),
        "updated_at": format_time(item.updated_at),
    })
}

fn product_media_json(item: ProductMediaAssetItem) -> serde_json::Value {
    let ProductMediaAssetItem {
        asset,
        product_id,
        scope,
        alt_text,
        position,
        archived_at,
    } = item;
    let mut value = media_asset_json(asset);
    value["product_id"] = json!(product_id.as_uuid());
    match scope {
        chaos_core::contracts::ProductMediaScope::Product => {
            value["scope"] = json!("product");
        }
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
    value["alt_text"] = json!(alt_text);
    value["position"] = json!(position);
    value["archived_at"] = json!(archived_at.map(format_time));
    value
}

fn product_media_mutation_json(item: ProductMediaAssetItem, revision: i64) -> serde_json::Value {
    let mut value = product_media_json(item);
    value["revision"] = json!(revision);
    value
}

fn product_media_replacement_json(
    output: chaos_core::catalog::ProductMediaReplacementOutput,
) -> serde_json::Value {
    json!({
        "revision": output.revision,
        "items": output.items.into_iter().map(product_media_json).collect::<Vec<_>>(),
    })
}

fn product_media_list_json(items: Vec<ProductMediaAssetItem>) -> serde_json::Value {
    json!({
        "items": items.into_iter().map(product_media_json).collect::<Vec<_>>(),
    })
}

fn review_media_json(item: ReviewMediaAssetItem) -> serde_json::Value {
    let ReviewMediaAssetItem {
        asset,
        review_id,
        alt_text,
        position,
        archived_at,
    } = item;
    let mut value = media_asset_json(asset);
    value["review_id"] = json!(review_id.as_uuid());
    value["alt_text"] = json!(alt_text);
    value["position"] = json!(position);
    value["archived_at"] = json!(archived_at.map(format_time));
    value
}

fn product_meta_media_json(item: ProductMetaMediaAssetItem) -> serde_json::Value {
    let ProductMetaMediaAssetItem {
        asset,
        product_id,
        meta_path,
        alt_text,
        archived_at,
    } = item;
    let mut value = media_asset_json(asset);
    value["product_id"] = json!(product_id.as_uuid());
    value["meta_path"] = json!(meta_path);
    value["alt_text"] = json!(alt_text);
    value["archived_at"] = json!(archived_at.map(format_time));
    value
}

fn prepared_media_result(
    media_asset: serde_json::Value,
    upload: MediaUploadRequest,
    next_step: &'static str,
) -> CallToolResult {
    CallToolResult::structured(json!({
        "media_asset": media_asset,
        "upload": upload_request_json(upload),
        "next_step": next_step,
    }))
}

fn refreshed_media_result(
    media_asset_id: MediaAssetId,
    upload: MediaUploadRequest,
    next_step: &'static str,
) -> CallToolResult {
    CallToolResult::structured(json!({
        "media_asset_id": media_asset_id.as_uuid(),
        "status": "pending_upload",
        "upload": upload_request_json(upload),
        "next_step": next_step,
    }))
}

fn upload_request_json(upload: MediaUploadRequest) -> serde_json::Value {
    let headers = upload
        .headers
        .into_iter()
        .map(|(name, value)| (name, serde_json::Value::String(value)))
        .collect::<serde_json::Map<_, _>>();
    json!({
        "method": upload.method,
        "url": upload.url,
        "headers": headers,
        "expires_at": format_time(upload.expires_at),
    })
}

fn parse_media_asset_id(value: &str) -> Result<MediaAssetId, CallToolResult> {
    parse_uuid_field(value, "media_asset_id").map(MediaAssetId::from_uuid)
}

fn parse_product_id(value: &str) -> Result<ProductId, CallToolResult> {
    parse_uuid_field(value, "product_id").map(ProductId::from_uuid)
}

fn parse_product_option_id(value: &str) -> Result<ProductOptionId, CallToolResult> {
    parse_uuid_field(value, "option_id").map(ProductOptionId::from_uuid)
}

fn parse_product_option_value_id(value: &str) -> Result<ProductOptionValueId, CallToolResult> {
    parse_uuid_field(value, "option_value_id").map(ProductOptionValueId::from_uuid)
}

fn parse_product_variant_id(value: &str) -> Result<ProductVariantId, CallToolResult> {
    parse_uuid_field(value, "product_variant_id").map(ProductVariantId::from_uuid)
}

fn parse_review_id(value: &str) -> Result<ReviewId, CallToolResult> {
    parse_uuid_field(value, "review_id").map(ReviewId::from_uuid)
}

fn parse_uuid_field(value: &str, field: &'static str) -> Result<uuid::Uuid, CallToolResult> {
    uuid::Uuid::parse_str(value).map_err(|_| {
        CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": format!("{field} must be a valid UUID"),
        }))
    })
}

fn parse_product_media_items(
    items: Vec<ProductMediaItemParams>,
) -> Result<Vec<ProductMediaItemInput>, CallToolResult> {
    items
        .into_iter()
        .map(|item| {
            Ok(ProductMediaItemInput {
                media_asset_id: parse_media_asset_id(&item.media_asset_id)?,
                alt_text: item.alt_text,
                position: item.position,
            })
        })
        .collect()
}

fn format_time(value: time::OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_upload_request_is_model_visible_content() {
        let url = "https://uploads.example.test/media/asset";
        let result = refreshed_media_result(
            MediaAssetId::from_uuid(uuid::Uuid::nil()),
            MediaUploadRequest {
                method: "PUT",
                url: url.into(),
                headers: vec![("content-type".into(), "image/webp".into())],
                expires_at: time::OffsetDateTime::UNIX_EPOCH,
            },
            "The MCP Host must upload the bytes.",
        );
        let wire = serde_json::to_value(result).unwrap();
        let model_text = wire["content"][0]["text"].as_str().unwrap();

        assert!(model_text.contains(url));
        assert_eq!(
            wire["structuredContent"]["media_asset_id"],
            serde_json::json!(uuid::Uuid::nil())
        );
        assert_eq!(wire["structuredContent"]["upload"]["url"], url);
        assert!(wire.get("_meta").is_none());
        assert_eq!(
            wire["structuredContent"]["upload"]["headers"]["content-type"],
            "image/webp"
        );
    }
}
