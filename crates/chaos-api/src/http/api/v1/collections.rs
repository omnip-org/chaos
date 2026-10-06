//! Storefront collection listing and detail endpoints.

use axum::{Router, extract::State, routing::get};
use chaos_core::contracts::StorefrontCollectionItem;
use chaos_domain::catalog::CollectionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::shared::pagination::{
    CursorKind, decode_cursor, encode_cursor, page_limit, page_meta,
};
use crate::http::{ApiError, ApiPath, ApiQuery, ApiResponse, ApiState, PublishableChannel};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/collections", get(list_collections))
        .route("/collections/{handle}", get(get_collection))
}

// ===== request contracts =====

#[derive(Deserialize)]
struct ListCollectionsQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize)]
struct CollectionPath {
    handle: String,
}

// ===== response contracts =====

#[derive(Serialize)]
struct CollectionResponse {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    product_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

impl From<StorefrontCollectionItem> for CollectionResponse {
    fn from(value: StorefrontCollectionItem) -> Self {
        Self {
            id: value.id.as_uuid(),
            handle: value.handle,
            title: value.title,
            description: value.description,
            product_count: value.product_count,
            metadata: value.metadata,
        }
    }
}

// ===== GET /collections =====

async fn list_collections(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiQuery(query): ApiQuery<ListCollectionsQuery>,
) -> Result<ApiResponse<Vec<CollectionResponse>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|value| decode_cursor(value, CursorKind::Collection))
        .transpose()?
        .map(CollectionId::from_uuid);
    let page = state
        .storefront_collections
        .list(&actor, after, limit)
        .await?;
    let next_cursor = page
        .has_more
        .then(|| {
            page.items
                .last()
                .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::Collection))
        })
        .flatten();
    Ok(
        ApiResponse::ok(page.items.into_iter().map(Into::into).collect())
            .with_meta(page_meta(page.has_more, next_cursor)),
    )
}

// ===== GET /collections/{handle} =====

async fn get_collection(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiPath(path): ApiPath<CollectionPath>,
) -> Result<ApiResponse<CollectionResponse>, ApiError> {
    let collection = state
        .storefront_collections
        .get(&actor, &path.handle)
        .await?;
    Ok(ApiResponse::ok(collection.into()))
}
