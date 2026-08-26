use chaos_core::{
    catalog::{CreateMediaAssetInput, MediaAssetActionInput, RefreshMediaUploadInput},
    contracts::{MediaAssetItem, MediaUploadRequest},
};
use chaos_domain::catalog::{MediaAssetId, ProductId, ProductVariantId};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::{CallToolResult, MetaObject},
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
pub struct PrepareProductMediaUploadParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// Optional product variant UUID, if this image is specific to one variant.
    #[serde(default)]
    pub product_variant_id: Option<String>,
    /// Exact byte size of the file that the Host will upload.
    pub byte_size: u64,
    /// Lowercase SHA-256 hex digest of the exact file bytes.
    pub sha256_hex: String,
    /// Original file name, e.g. "hero.webp".
    pub file_name: String,
    /// MIME type, e.g. "image/webp", "image/jpeg", "image/png".
    pub media_type: String,
    /// Alt text for accessibility and SEO.
    #[serde(default)]
    pub alt_text: String,
    /// Display order among the product's media (0-99).
    #[serde(default)]
    pub position: u16,
    /// Must be explicitly set to true. This creates a pending Media Asset.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RefreshProductMediaUploadParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// The pending Media Asset's UUID.
    pub media_asset_id: String,
    /// Must be explicitly set to true. This issues a new short-lived upload request.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CompleteProductMediaUploadParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// The pending Media Asset's UUID.
    pub media_asset_id: String,
    /// Must be explicitly set to true. This marks the Media Asset ready after verification.
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListProductMediaParams {
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
    /// The media asset's UUID.
    pub media_asset_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = media_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Prepare a direct upload for a product image in the selected Store. \
                        Provide file metadata and the exact lowercase SHA-256 digest, but do not \
                        send image bytes. Returns a pending media_asset_id and a short-lived \
                        presigned PUT request for the MCP Host to use directly with object \
                        storage. After the Host uploads the bytes, call \
                        complete_product_media_upload. Requires confirm: true."
    )]
    async fn prepare_product_media_upload(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<PrepareProductMediaUploadParams>,
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
        let product_variant_id = match params.product_variant_id.as_deref() {
            Some(value) => match parse_uuid_field(value, "product_variant_id") {
                Ok(id) => Some(ProductVariantId::from_uuid(id)),
                Err(result) => return Ok(result),
            },
            None => None,
        };
        let now = self.state.clock.now();

        match self
            .state
            .media_administration
            .create(CreateMediaAssetInput {
                actor,
                store_id,
                product_id,
                product_variant_id,
                file_name: params.file_name,
                media_type: params.media_type,
                byte_size: params.byte_size,
                sha256_hex: params.sha256_hex,
                alt_text: params.alt_text,
                position: params.position,
                now,
            })
            .await
        {
            Ok(created) => Ok(prepared_media_result(created.asset, created.upload)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Refresh the short-lived direct-upload request for a pending product \
                        Media Asset. The MCP Host must use the returned PUT request to upload the \
                        exact bytes whose metadata was supplied during preparation. Requires \
                        confirm: true."
    )]
    async fn refresh_product_media_upload(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RefreshProductMediaUploadParams>,
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
        let media_asset_id = match parse_uuid_field(&params.media_asset_id, "media_asset_id") {
            Ok(id) => MediaAssetId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let now = self.state.clock.now();

        match self
            .state
            .media_administration
            .refresh_upload(RefreshMediaUploadInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                now,
            })
            .await
        {
            Ok(upload) => Ok(refreshed_media_result(media_asset_id, upload)),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Complete a direct product media upload. Call this only after the MCP Host \
                        has PUT the exact file bytes to the prepared upload request. Chaos \
                        verifies object existence, MIME type, byte size, and SHA-256 before \
                        marking the Media Asset ready. Requires confirm: true."
    )]
    async fn complete_product_media_upload(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CompleteProductMediaUploadParams>,
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
        let media_asset_id = match parse_uuid_field(&params.media_asset_id, "media_asset_id") {
            Ok(id) => MediaAssetId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let now = self.state.clock.now();

        match self
            .state
            .media_administration
            .complete(MediaAssetActionInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                now,
            })
            .await
        {
            Ok(item) => Ok(text_result(media_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List media assets for a product in the selected Store, \
                        including pending and archived ones."
    )]
    async fn list_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListProductMediaParams>,
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
            .media_administration
            .list(actor, store_id, product_id)
            .await
        {
            Ok(items) => Ok(text_result(json!({
                "items": items.into_iter().map(media_json).collect::<Vec<_>>(),
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Archive a media asset for a product in the selected Store. \
                        Requires confirm: true."
    )]
    async fn archive_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ArchiveProductMediaParams>,
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
        let media_asset_id = match parse_uuid_field(&params.media_asset_id, "media_asset_id") {
            Ok(id) => MediaAssetId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let now = self.state.clock.now();

        match self
            .state
            .media_administration
            .archive(MediaAssetActionInput {
                actor,
                store_id,
                product_id,
                media_asset_id,
                now,
            })
            .await
        {
            Ok(item) => Ok(text_result(media_json(item))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn media_json(item: MediaAssetItem) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "product_id": item.product_id.as_uuid(),
        "product_variant_id": item.product_variant_id.map(|id| id.as_uuid()),
        "file_name": item.file_name,
        "media_type": item.media_type,
        "kind": item.kind.as_str(),
        "byte_size": item.byte_size,
        "sha256_hex": item.sha256_hex,
        "alt_text": item.alt_text,
        "position": item.position,
        "status": item.status.as_str(),
        "public_url": item.public_url,
        "created_at": format_time(item.created_at),
        "updated_at": format_time(item.updated_at),
    })
}

fn prepared_media_result(item: MediaAssetItem, upload: MediaUploadRequest) -> CallToolResult {
    CallToolResult::structured(json!({
        "media_asset": media_json(item),
        "next_step": "The MCP Host must PUT the original file bytes using the upload request in _meta, then call complete_product_media_upload.",
    }))
    .with_meta(Some(media_upload_meta(upload)))
}

fn refreshed_media_result(
    media_asset_id: MediaAssetId,
    upload: MediaUploadRequest,
) -> CallToolResult {
    CallToolResult::structured(json!({
        "media_asset_id": media_asset_id.as_uuid(),
        "status": "pending_upload",
        "next_step": "The MCP Host must PUT the original file bytes using the upload request in _meta, then call complete_product_media_upload.",
    }))
    .with_meta(Some(media_upload_meta(upload)))
}

fn media_upload_meta(upload: MediaUploadRequest) -> MetaObject {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "com.omniporg.chaos/media-upload".into(),
        upload_request_json(upload),
    );
    MetaObject(meta)
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
    fn direct_upload_request_is_host_metadata_not_model_content() {
        let url = "https://uploads.example.test/media/asset";
        let result = refreshed_media_result(
            MediaAssetId::from_uuid(uuid::Uuid::nil()),
            MediaUploadRequest {
                method: "PUT",
                url: url.into(),
                headers: vec![("content-type".into(), "image/webp".into())],
                expires_at: time::OffsetDateTime::UNIX_EPOCH,
            },
        );
        let wire = serde_json::to_value(result).unwrap();
        let model_text = wire["content"][0]["text"].as_str().unwrap();

        assert!(!model_text.contains(url));
        assert_eq!(
            wire["structuredContent"]["media_asset_id"],
            serde_json::json!(uuid::Uuid::nil())
        );
        assert_eq!(wire["_meta"]["com.omniporg.chaos/media-upload"]["url"], url);
        assert_eq!(
            wire["_meta"]["com.omniporg.chaos/media-upload"]["headers"]["content-type"],
            "image/webp"
        );
    }
}
