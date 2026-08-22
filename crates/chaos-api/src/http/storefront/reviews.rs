use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::{
    ApplicationError,
    catalog::SubmitReviewInput,
    ports::{IdempotencyRequest, ReviewSummary},
};
use chaos_domain::catalog::{ProductId, ReviewId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::http::shared::pagination::{
    CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta,
};
use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, StorefrontMachine,
};

pub(crate) fn storefront_routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/products/{product_id}/reviews",
            post(submit_review).get(list_product_reviews),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct ProductPath {
    product_id: Uuid,
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
        .map(|value| decode_cursor(value, CursorKind::Review))
        .transpose()?
        .map(ReviewId::from_uuid);
    let page = state
        .storefront_reviews
        .list_for_product(&actor, ProductId::from_uuid(path.product_id), after, limit)
        .await?;
    let next_cursor = page
        .has_more
        .then(|| {
            page.items
                .iter()
                .rev()
                .find(|item| item.parent_review_id.is_none())
                .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::Review))
        })
        .flatten();
    Ok(ApiResponse::ok(nest_replies(page.items)).with_meta(page_meta(page.has_more, next_cursor)))
}

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
        parent_id: item.parent_review_id.map(ReviewId::as_uuid),
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

fn mutation<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(value)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}
