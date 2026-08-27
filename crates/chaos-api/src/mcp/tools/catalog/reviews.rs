use chaos_core::catalog::{
    AddReviewReplyInput, ApproveReviewInput, CreateManualReviewInput, RejectReviewInput,
};
use chaos_domain::catalog::{ProductId, ReviewId, ReviewStatus};
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
#[serde(rename_all = "snake_case")]
pub enum ReviewStatusParam {
    Pending,
    Approved,
    Rejected,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListReviewsParams {
    /// The Store UUID to inspect.
    pub store_id: String,
    /// Filter by review status: pending, approved, or rejected. Defaults to pending.
    #[serde(default)]
    pub status: Option<ReviewStatusParam>,
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of reviews to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct CreateManualReviewParams {
    /// The Store UUID containing the Product.
    pub store_id: String,
    /// The Product UUID the customer reviewed.
    pub product_id: String,
    /// The customer's explicit rating, from 1 to 5. Do not infer this from prose.
    pub rating: u8,
    #[serde(default)]
    pub title: Option<String>,
    /// The review text transcribed from the customer's message.
    pub content: String,
    pub author_name: String,
    #[serde(default)]
    pub author_email: Option<String>,
    /// The source channel, e.g. "wechat", "instagram_dm", "email", or "phone".
    pub source_channel: String,
    /// An internal conversation or ticket reference. Do not put the full private message here.
    #[serde(default)]
    pub source_reference: Option<String>,
    /// Must be explicitly set to true after the customer agreed to public display.
    pub publication_consent_confirmed: bool,
    /// Must be explicitly set to true. This creates a pending review.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ApproveReviewParams {
    /// The Store UUID containing the review.
    pub store_id: String,
    /// The review's UUID.
    pub review_id: String,
    /// Whether to mark the review as left by a verified buyer.
    #[serde(default)]
    pub verified_buyer: bool,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RejectReviewParams {
    /// The Store UUID containing the review.
    pub store_id: String,
    /// The review's UUID.
    pub review_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AddReviewReplyParams {
    /// The Store UUID containing the review.
    pub store_id: String,
    /// The UUID of the review being replied to.
    pub review_id: String,
    pub content: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
}

#[tool_router(router = reviews_tool_router, vis = "pub(in crate::mcp::tools)")]
impl ChaosMcp {
    #[tool(
        description = "Create a customer review imported manually from an external channel such \
                        as a private message, email, or phone call. The review starts pending and \
                        remains invisible on the Storefront until approved. The rating must be \
                        explicitly supplied; do not infer it. This tool does not mark the customer \
                        as a verified buyer. Set publication_consent_confirmed: true only after \
                        the customer agreed that the review and attached images may be published. \
                        Requires confirm: true."
    )]
    async fn create_manual_review(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<CreateManualReviewParams>,
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
        let store_id = actor.store_id();
        match self
            .state
            .review_administration
            .create_manual(CreateManualReviewInput {
                actor,
                store_id,
                product_id,
                rating: params.rating,
                title: params.title,
                content: params.content,
                author_name: params.author_name,
                author_email: params.author_email,
                source_channel: params.source_channel,
                source_reference: params.source_reference,
                publication_consent_confirmed: params.publication_consent_confirmed,
                now: self.state.clock.now(),
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({
                "id": id.as_uuid(),
                "status": "pending",
                "origin": "manual",
                "next_step": "Optionally prepare and complete images with the generic media upload tools, attach them with attach_review_media, then approve_review after moderation.",
            }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "List reviews in the selected Store, filtered by status \
                        (defaults to pending). Paginated; use the returned next_cursor for \
                        more pages."
    )]
    async fn list_reviews(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ListReviewsParams>,
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
        let status = match params.status.unwrap_or(ReviewStatusParam::Pending) {
            ReviewStatusParam::Pending => ReviewStatus::Pending,
            ReviewStatusParam::Approved => ReviewStatus::Approved,
            ReviewStatusParam::Rejected => ReviewStatus::Rejected,
        };
        let after = match params.cursor.as_deref().map(parse_uuid_cursor) {
            Some(Ok(id)) => Some(ReviewId::from_uuid(id)),
            Some(Err(result)) => return Ok(result),
            None => None,
        };
        let limit = params.limit.unwrap_or(20);

        match self
            .state
            .review_administration
            .list_by_status(actor, store_id, status, after, limit)
            .await
        {
            Ok(page) => {
                let items = page
                    .items
                    .into_iter()
                    .map(review_summary)
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
        description = "Approve a pending review in the selected Store, making it \
                        visible on the product page. Requires confirm: true."
    )]
    async fn approve_review(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ApproveReviewParams>,
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
        let review_id = match parse_uuid_field(&params.review_id, "review_id") {
            Ok(id) => ReviewId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let now = self.state.clock.now();

        match self
            .state
            .review_administration
            .approve(ApproveReviewInput {
                actor,
                store_id,
                review_id,
                verified_buyer: params.verified_buyer,
                now,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(
        description = "Reject a pending review in the selected Store, keeping it \
                        hidden from the product page. Requires confirm: true."
    )]
    async fn reject_review(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RejectReviewParams>,
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
        let review_id = match parse_uuid_field(&params.review_id, "review_id") {
            Ok(id) => ReviewId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let now = self.state.clock.now();

        match self
            .state
            .review_administration
            .reject(RejectReviewInput {
                actor,
                store_id,
                review_id,
                now,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Add a staff reply to a review in the selected Store. \
                        Requires confirm: true.")]
    async fn add_review_reply(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AddReviewReplyParams>,
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
        let parent_review_id = match parse_uuid_field(&params.review_id, "review_id") {
            Ok(id) => ReviewId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let now = self.state.clock.now();

        match self
            .state
            .review_administration
            .add_reply(AddReviewReplyInput {
                actor,
                store_id,
                parent_review_id,
                content: params.content,
                now,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn review_summary(item: chaos_core::contracts::ReviewSummary) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "product_id": item.product_id.as_uuid(),
        "parent_review_id": item.parent_review_id.map(|id| id.as_uuid()),
        "rating": item.rating,
        "title": item.title,
        "content": item.content,
        "author_name": item.author_name,
        "author_email": item.author_email,
        "status": item.status.as_str(),
        "is_staff_reply": item.is_staff_reply,
        "verified_buyer": item.verified_buyer,
        "origin": item.origin.as_str(),
        "source_channel": item.source_channel,
        "source_reference": item.source_reference,
        "images": item.images.into_iter().map(review_image).collect::<Vec<_>>(),
        "created_at": format_time(item.created_at),
        "updated_at": format_time(item.updated_at),
    })
}

fn review_image(item: chaos_core::contracts::ReviewMediaSummary) -> serde_json::Value {
    json!({
        "id": item.id.as_uuid(),
        "media_type": item.media_type,
        "kind": item.kind.as_str(),
        "alt_text": item.alt_text,
        "position": item.position,
        "status": item.status.as_str(),
        "public_url": item.public_url,
    })
}

fn parse_uuid_cursor(value: &str) -> Result<uuid::Uuid, CallToolResult> {
    parse_uuid_field(value, "cursor")
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
