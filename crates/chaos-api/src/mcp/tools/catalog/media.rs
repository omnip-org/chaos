use chaos_core::{
    catalog::{
        ArchiveMediaAssetInput, ArchiveProductMediaInput, ArchiveProductMetaMediaInput,
        ArchiveReviewMediaInput, AttachProductMediaInput, AttachProductMetaMediaInput,
        AttachReviewMediaInput, CompleteMediaUploadInput, CreateMediaUploadInput,
        RefreshMediaUploadInput,
    },
    contracts::{
        MediaAssetItem, MediaUploadRequest, ProductMediaAssetItem, ProductMetaMediaAssetItem,
        ReviewMediaAssetItem,
    },
};
use chaos_domain::catalog::{MediaAssetId, ProductId, ProductVariantId, ReviewId};
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
pub struct AttachProductMediaParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// Optional product variant UUID, if this media is specific to one variant.
    #[serde(default)]
    pub product_variant_id: Option<String>,
    /// A ready Media Asset's UUID.
    pub media_asset_id: String,
    /// Alt text for accessibility and SEO.
    #[serde(default)]
    pub alt_text: String,
    /// Display order among the product's media (0-99).
    #[serde(default)]
    pub position: u16,
    /// Must be explicitly set to true. This changes catalog presentation.
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
    /// Must be explicitly set to true. This removes the Product attachment.
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
    /// Must be explicitly set to true. This removes the metadata reference.
    pub confirm: bool,
}

#[tool_router(router = media_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
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
        description = "Attach a ready reusable Media Asset to a Product gallery or a specific \
                        Product Variant. The asset must be prepared and completed first. Requires \
                        confirm: true."
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
        let product_variant_id = match params.product_variant_id.as_deref() {
            Some(value) => match parse_product_variant_id(value) {
                Ok(id) => Some(id),
                Err(result) => return Ok(result),
            },
            None => None,
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
                product_variant_id,
                media_asset_id,
                alt_text: params.alt_text,
                position: params.position,
            })
            .await
        {
            Ok(item) => Ok(text_result(product_media_json(item))),
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
                        transactionally. Replacing an existing path archives its previous link. \
                        Requires confirm: true."
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
            })
            .await
        {
            Ok(item) => Ok(text_result(product_meta_media_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List all Product gallery Media attachments in the selected Store, \
                        including pending and archived links."
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
            })
            .await
        {
            Ok(item) => Ok(text_result(product_media_json(item))),
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
        description = "Archive a Product metadata Media attachment and remove its metadata \
                        reference if it still points at the same asset. Requires confirm: true."
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
            })
            .await
        {
            Ok(item) => Ok(text_result(product_meta_media_json(item))),
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
        product_variant_id,
        alt_text,
        position,
        archived_at,
    } = item;
    let mut value = media_asset_json(asset);
    value["product_id"] = json!(product_id.as_uuid());
    value["product_variant_id"] = json!(product_variant_id.map(|id| id.as_uuid()));
    value["alt_text"] = json!(alt_text);
    value["position"] = json!(position);
    value["archived_at"] = json!(archived_at.map(format_time));
    value
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
