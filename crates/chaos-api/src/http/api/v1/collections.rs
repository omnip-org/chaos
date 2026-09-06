//! Storefront collection listing and detail endpoints.

use axum::{Router, extract::State, routing::get};
use chaos_core::contracts::StorefrontCollectionItem;
use chaos_domain::catalog::CollectionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::shared::pagination::{
    CursorKind, decode_cursor, encode_cursor, page_limit, page_meta,
};
use crate::http::{ApiPath, ApiQuery, ApiResponse, ApiState, PublishableChannel};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/collections", get(list::handler))
        .route("/collections/{handle}", get(get_one::handler))
}

// ===== shared wire types & mappers =====

#[derive(Serialize)]
struct CollectionData {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    product_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

fn collection_data(value: StorefrontCollectionItem) -> CollectionData {
    CollectionData {
        id: value.id.as_uuid(),
        handle: value.handle,
        title: value.title,
        description: value.description,
        product_count: value.product_count,
        metadata: value.metadata,
    }
}

// ===== GET /collections =====

mod list {
    use super::*;

    #[derive(Deserialize)]
    pub(super) struct ListCollectionsQuery {
        cursor: Option<String>,
        limit: Option<u16>,
    }

    pub(super) async fn handler(
        State(state): State<ApiState>,
        PublishableChannel(actor): PublishableChannel,
        ApiQuery(query): ApiQuery<ListCollectionsQuery>,
    ) -> Result<ApiResponse<Vec<CollectionData>>, crate::http::ApiError> {
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
            ApiResponse::ok(page.items.into_iter().map(collection_data).collect())
                .with_meta(page_meta(page.has_more, next_cursor)),
        )
    }
}

// ===== GET /collections/{handle} =====

mod get_one {
    use super::*;

    #[derive(Deserialize)]
    pub(super) struct CollectionPath {
        handle: String,
    }

    pub(super) async fn handler(
        State(state): State<ApiState>,
        PublishableChannel(actor): PublishableChannel,
        ApiPath(path): ApiPath<CollectionPath>,
    ) -> Result<ApiResponse<CollectionData>, crate::http::ApiError> {
        Ok(ApiResponse::ok(collection_data(
            state
                .storefront_collections
                .get(&actor, &path.handle)
                .await?,
        )))
    }
}
