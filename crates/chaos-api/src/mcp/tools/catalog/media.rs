use base64::Engine as _;
use chaos_core::{
    catalog::{CreateMediaAssetInput, MediaAssetActionInput},
    contracts::MediaAssetItem,
};
use chaos_domain::catalog::{MediaAssetId, ProductId, ProductVariantId};
use rmcp::{
    ErrorData,
    handler::server::{common::Extension, wrapper::Parameters},
    model::CallToolResult,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;

use crate::mcp::tools::ChaosMcp;
use crate::mcp::{
    error::{text_result, tool_error},
    mutation::require_confirmation,
};

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UploadProductMediaParams {
    /// The Store UUID containing the product.
    pub store_id: String,
    /// The product's UUID.
    pub product_id: String,
    /// Optional product variant UUID, if this image is specific to one variant.
    #[serde(default)]
    pub product_variant_id: Option<String>,
    /// Base64-encoded image bytes (no data: URI prefix).
    pub data_base64: String,
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
    /// Must be explicitly set to true. This action affects live store data.
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
        description = "Upload an image for a product in the selected Store, in \
                        one call. Provide the image as base64-encoded bytes (data_base64); this \
                        tool computes the checksum, creates the Media Asset record, uploads the \
                        bytes to storage, and marks it ready — no separate presigned-URL steps \
                        required. Requires confirm: true."
    )]
    async fn upload_product_media(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<UploadProductMediaParams>,
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
        let bytes = match base64::engine::general_purpose::STANDARD.decode(&params.data_base64) {
            Ok(bytes) => bytes,
            Err(_) => {
                return Ok(CallToolResult::structured_error(json!({
                    "code": "invalid_params",
                    "message": "data_base64 must be valid standard base64",
                })));
            }
        };
        if bytes.is_empty() {
            return Ok(CallToolResult::structured_error(json!({
                "code": "invalid_params",
                "message": "data_base64 must decode to a non-empty byte string",
            })));
        }
        let byte_size = bytes.len() as u64;
        let sha256_hex = hex_digest(&bytes);
        let now = self.state.clock.now();
        let created = match self
            .state
            .media_administration
            .create(CreateMediaAssetInput {
                actor: actor.clone(),
                store_id,
                product_id,
                product_variant_id,
                file_name: params.file_name,
                media_type: params.media_type,
                byte_size,
                sha256_hex,
                alt_text: params.alt_text,
                position: params.position,
                now,
            })
            .await
        {
            Ok(created) => created,
            Err(error) => return Ok(tool_error(error)),
        };

        let client = match self.http_client() {
            Ok(client) => client,
            Err(result) => return Ok(result),
        };
        let mut request = client.request(
            match created.upload.method {
                "PUT" => reqwest::Method::PUT,
                other => {
                    return Ok(CallToolResult::structured_error(json!({
                        "code": "upload_failed",
                        "message": format!("unsupported upload method: {other}"),
                    })));
                }
            },
            &created.upload.url,
        );
        for (name, value) in &created.upload.headers {
            request = request.header(name, value);
        }
        let response = match request.body(bytes).send().await {
            Ok(response) => response,
            Err(error) => {
                return Ok(CallToolResult::structured_error(json!({
                    "code": "upload_failed",
                    "message": format!("uploading Media bytes failed: {error}"),
                })));
            }
        };
        if !response.status().is_success() {
            return Ok(CallToolResult::structured_error(json!({
                "code": "upload_failed",
                "message": format!(
                    "Media storage rejected the upload with status {}",
                    response.status()
                ),
            })));
        }

        match self
            .state
            .media_administration
            .complete(MediaAssetActionInput {
                actor,
                store_id,
                product_id,
                media_asset_id: created.asset.id,
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

impl ChaosMcp {
    fn http_client(&self) -> Result<reqwest::Client, CallToolResult> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder().build().map_err(|error| {
            CallToolResult::structured_error(json!({
                "code": "upload_failed",
                "message": format!("failed to construct the upload HTTP client: {error}"),
            }))
        })
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

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
