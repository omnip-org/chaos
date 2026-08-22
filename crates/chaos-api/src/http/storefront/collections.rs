use axum::{Router, extract::State, routing::get};
use chaos_application::ports::StorefrontCollectionItem;
use chaos_domain::catalog::CollectionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::shared::pagination::{
    CursorKind, decode_cursor, encode_cursor, page_limit, page_meta,
};
use crate::http::{ApiPath, ApiQuery, ApiResponse, ApiState, StorefrontMachine};

pub fn storefront_routes() -> Router<ApiState> {
    Router::new()
        .route("/collections", get(list_storefront_collections))
        .route("/collections/{handle}", get(get_storefront_collection))
}

#[derive(Deserialize)]
struct HandlePath {
    handle: String,
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
    locale: Option<String>,
}

#[derive(Deserialize)]
struct LocaleQuery {
    locale: Option<String>,
}

#[derive(Serialize)]
struct StorefrontCollectionData {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    product_count: u32,
    locale: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

async fn list_storefront_collections(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<StorefrontCollectionData>>, crate::http::ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|value| decode_cursor(value, CursorKind::Collection))
        .transpose()?
        .map(CollectionId::from_uuid);
    let page = state
        .storefront_collections
        .list(&actor, query.locale.as_deref(), after, limit)
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
        ApiResponse::ok(page.items.into_iter().map(storefront_data).collect())
            .with_meta(page_meta(page.has_more, next_cursor)),
    )
}

async fn get_storefront_collection(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiPath(path): ApiPath<HandlePath>,
    ApiQuery(query): ApiQuery<LocaleQuery>,
) -> Result<ApiResponse<StorefrontCollectionData>, crate::http::ApiError> {
    Ok(ApiResponse::ok(storefront_data(
        state
            .storefront_collections
            .get(&actor, query.locale.as_deref(), &path.handle)
            .await?,
    )))
}

fn storefront_data(value: StorefrontCollectionItem) -> StorefrontCollectionData {
    StorefrontCollectionData {
        id: value.id.as_uuid(),
        handle: value.handle,
        title: value.title,
        description: value.description,
        product_count: value.product_count,
        locale: value.locale.as_str().into(),
        metadata: value.metadata,
    }
}
