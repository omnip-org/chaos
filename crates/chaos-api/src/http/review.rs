use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    catalog::{AddReviewReplyInput, ApproveReviewInput, RejectReviewInput, SubmitReviewInput},
    ports::{AdminActor, IdempotencyRequest, ReviewSummary},
};
use chaos_domain::{
    catalog::{ProductId, ReviewId, ReviewStatus},
    merchant::{MerchantAccountId, StoreId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, MerchantContext,
    StorefrontMachine,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
};

pub fn admin_routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/reviews",
            get(list_reviews),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/reviews/{review_id}/approve",
            post(approve_review),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/reviews/{review_id}/reject",
            post(reject_review),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/reviews/{review_id}/replies",
            post(add_reply),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

pub fn storefront_routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/products/{product_id}/reviews",
            post(submit_review).get(list_product_reviews),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}
#[derive(Deserialize)]
struct ReviewPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    review_id: Uuid,
}
#[derive(Deserialize)]
struct ProductPath {
    product_id: Uuid,
}
#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
}
#[derive(Deserialize)]
struct StorefrontListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubmitReviewBody {
    rating: u8,
    #[serde(default)]
    title: Option<String>,
    content: String,
    author_name: String,
    #[serde(default)]
    author_email: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ApproveReviewBody {
    verified_buyer: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AddReplyBody {
    content: String,
}

#[derive(Serialize)]
struct MutationData {
    id: Uuid,
}

#[derive(Serialize)]
struct ReviewData {
    id: Uuid,
    product_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<Uuid>,
    author_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    author_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    content: String,
    /// Always empty in this release — review-photo uploads are not yet built;
    /// see ADR 0023.
    images: Vec<String>,
    status: &'static str,
    is_staff_reply: bool,
    verified_buyer: bool,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    replies: Vec<ReviewData>,
}

async fn submit_review(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StorefrontMachine(actor): StorefrontMachine,
    ApiPath(path): ApiPath<ProductPath>,
    ApiJson(body): ApiJson<SubmitReviewBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    let request = mutation(&headers, &(path.product_id, &body))?;
    let id = state
        .review_administration
        .submit(SubmitReviewInput {
            actor,
            product_id: ProductId::from_uuid(path.product_id),
            rating: body.rating,
            title: body.title,
            content: body.content,
            author_name: body.author_name,
            author_email: body.author_email,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(MutationData { id: id.as_uuid() }))
}

async fn list_product_reviews(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiPath(path): ApiPath<ProductPath>,
    ApiQuery(query): ApiQuery<StorefrontListQuery>,
) -> Result<ApiResponse<Vec<ReviewData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|v| decode_cursor(v, CursorKind::StorefrontReview))
        .transpose()?
        .map(ReviewId::from_uuid);
    let page = state
        .storefront_reviews
        .list_for_product(&actor, ProductId::from_uuid(path.product_id), after, limit)
        .await?;
    let next = page
        .has_more
        .then(|| {
            page.items
                .iter()
                .rev()
                .find(|item| item.parent_review_id.is_none())
                .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::StorefrontReview))
        })
        .flatten();
    Ok(ApiResponse::ok(nest_replies(page.items)).with_meta(page_meta(page.has_more, next)))
}

async fn list_reviews(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<ReviewData>>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let status = match query.status.as_deref() {
        None => ReviewStatus::Pending,
        Some(value) => ReviewStatus::parse(value).ok_or(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "status",
                reason: "must be pending, approved, or rejected".into(),
            }],
        })?,
    };
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|v| decode_cursor(v, CursorKind::Review))
        .transpose()?
        .map(ReviewId::from_uuid);
    let page = state
        .review_administration
        .list_by_status(
            AdminActor::Merchant(actor),
            StoreId::from_uuid(path.store_id),
            status,
            after,
            limit,
        )
        .await?;
    let next = page
        .has_more
        .then(|| {
            page.items
                .last()
                .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::Review))
        })
        .flatten();
    Ok(
        ApiResponse::ok(page.items.into_iter().map(review_data).collect())
            .with_meta(page_meta(page.has_more, next)),
    )
}

async fn approve_review(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ReviewPath>,
    ApiJson(body): ApiJson<ApproveReviewBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let request = mutation(&headers, &(path.store_id, path.review_id, &body))?;
    let id = state
        .review_administration
        .approve(ApproveReviewInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            review_id: ReviewId::from_uuid(path.review_id),
            verified_buyer: body.verified_buyer,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn reject_review(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ReviewPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let request = mutation(&headers, &(path.store_id, path.review_id, "reject"))?;
    let id = state
        .review_administration
        .reject(RejectReviewInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            review_id: ReviewId::from_uuid(path.review_id),
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn add_reply(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ReviewPath>,
    ApiJson(body): ApiJson<AddReplyBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let request = mutation(&headers, &(path.store_id, path.review_id, &body))?;
    let id = state
        .review_administration
        .add_reply(AddReviewReplyInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            parent_review_id: ReviewId::from_uuid(path.review_id),
            content: body.content,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(MutationData { id: id.as_uuid() }))
}

/// The repository returns a flat, ordered list: each top-level review immediately
/// followed by its own approved replies (a reply never has replies of its own).
/// This regroups that flat list into the nested wire shape the Storefront expects.
fn nest_replies(items: Vec<ReviewSummary>) -> Vec<ReviewData> {
    let mut result: Vec<ReviewData> = Vec::new();
    for item in items {
        let is_reply = item.parent_review_id.is_some();
        let data = review_data(item);
        if is_reply {
            if let Some(parent) = result.last_mut() {
                parent.replies.push(data);
            }
        } else {
            result.push(data);
        }
    }
    result
}

fn review_data(item: ReviewSummary) -> ReviewData {
    ReviewData {
        id: item.id.as_uuid(),
        product_id: item.product_id.as_uuid(),
        parent_id: item.parent_review_id.map(|id| id.as_uuid()),
        author_name: item.author_name,
        author_email: item.author_email,
        rating: item.rating,
        title: item.title,
        content: item.content,
        images: Vec::new(),
        status: item.status.as_str(),
        is_staff_reply: item.is_staff_reply,
        verified_buyer: item.verified_buyer,
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
        replies: Vec::new(),
    }
}

fn account(actual: MerchantAccountId, path: Uuid) -> Result<(), ApiError> {
    if actual.as_uuid() == path {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden.into())
    }
}

fn mutation<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(value).map_err(|e| ApplicationError::Unexpected(e.into()))?,
        )
        .into(),
    })
}
