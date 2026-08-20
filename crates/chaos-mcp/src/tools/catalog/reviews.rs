use chaos_application::catalog::{AddReviewReplyInput, ApproveReviewInput, RejectReviewInput};
use chaos_domain::catalog::{ReviewId, ReviewStatus};
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
pub struct ListReviewsParams {
    /// Filter by review status: pending, approved, or rejected. Defaults to pending.
    #[serde(default)]
    pub status: Option<String>,
    /// Opaque cursor from a previous page's `next_cursor`. Omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Maximum number of reviews to return (1-100). Defaults to 20.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ApproveReviewParams {
    /// The review's UUID.
    pub review_id: String,
    /// Whether to mark the review as left by a verified buyer.
    #[serde(default)]
    pub verified_buyer: bool,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RejectReviewParams {
    /// The review's UUID.
    pub review_id: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AddReviewReplyParams {
    /// The UUID of the review being replied to.
    pub review_id: String,
    pub content: String,
    /// Must be explicitly set to true. This action affects live store data.
    pub confirm: bool,
    /// A client-chosen key identifying this exact attempt.
    pub idempotency_key: String,
}

#[tool_router(router = reviews_tool_router, vis = "pub(in crate::tools)")]
impl ChaosMcp {
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
        let status = match params.status.as_deref().map(parse_review_status) {
            Some(Ok(status)) => status,
            Some(Err(result)) => return Ok(result),
            None => ReviewStatus::Pending,
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
                        visible on the product page. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn approve_review(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<ApproveReviewParams>,
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
        let review_id = match parse_uuid_field(&params.review_id, "review_id") {
            Ok(id) => ReviewId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .review_administration
            .approve(ApproveReviewInput {
                actor,
                store_id,
                review_id,
                verified_buyer: params.verified_buyer,
                idempotency,
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
                        hidden from the product page. Requires confirm: true and an \
                        idempotency_key."
    )]
    async fn reject_review(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<RejectReviewParams>,
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
        let review_id = match parse_uuid_field(&params.review_id, "review_id") {
            Ok(id) => ReviewId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .review_administration
            .reject(RejectReviewInput {
                actor,
                store_id,
                review_id,
                idempotency,
                now,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }

    #[tool(description = "Add a staff reply to a review in the selected Store. \
                        Requires confirm: true and an idempotency_key.")]
    async fn add_review_reply(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<AddReviewReplyParams>,
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
        let parent_review_id = match parse_uuid_field(&params.review_id, "review_id") {
            Ok(id) => ReviewId::from_uuid(id),
            Err(result) => return Ok(result),
        };
        let store_id = actor.store_id();
        let idempotency = idempotency_request(params.idempotency_key.clone(), &params);
        let now = self.state.clock.now();

        match self
            .state
            .review_administration
            .add_reply(AddReviewReplyInput {
                actor,
                store_id,
                parent_review_id,
                content: params.content,
                idempotency,
                now,
            })
            .await
        {
            Ok(id) => Ok(text_result(json!({ "id": id.as_uuid() }))),
            Err(error) => Ok(tool_error(error)),
        }
    }
}

fn review_summary(item: chaos_application::ports::ReviewSummary) -> serde_json::Value {
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
        "created_at": format_time(item.created_at),
        "updated_at": format_time(item.updated_at),
    })
}

fn parse_review_status(value: &str) -> Result<ReviewStatus, CallToolResult> {
    ReviewStatus::parse(value).ok_or_else(|| {
        CallToolResult::structured_error(json!({
            "code": "invalid_params",
            "message": "status must be one of: pending, approved, rejected",
        }))
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
