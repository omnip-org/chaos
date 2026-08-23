use axum::{
    Router,
    extract::{Path, Query, State},
    routing::{delete, post},
};
use chaos_domain::identity::AccessKeyId;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{ApiDateTime, ApiError, ApiJson, ApiResponse, ApiState, AuthenticatedUser};

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/access-keys",
            post(create_access_key).get(list_access_keys),
        )
        .route("/access-keys/{access_key_id}", delete(revoke_access_key))
}

#[derive(Deserialize)]
struct CreateAccessKeyBody {
    name: String,
}

#[derive(Deserialize)]
struct ListAccessKeysQuery {
    cursor: Option<Uuid>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct AccessKeyCreatedData {
    id: Uuid,
    name: String,
    key_identifier: String,
    display_suffix: String,
    secret: String,
}

#[derive(Serialize)]
struct AccessKeyData {
    id: Uuid,
    name: String,
    key_identifier: String,
    display_suffix: String,
    created_at: ApiDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revoked_at: Option<ApiDateTime>,
}

#[derive(Serialize)]
struct AccessKeyPageData {
    items: Vec<AccessKeyData>,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<Uuid>,
}

async fn create_access_key(
    State(state): State<ApiState>,
    AuthenticatedUser { user_id }: AuthenticatedUser,
    ApiJson(body): ApiJson<CreateAccessKeyBody>,
) -> Result<ApiResponse<AccessKeyCreatedData>, ApiError> {
    let output = state
        .access_key_management
        .create(user_id, body.name)
        .await?;
    Ok(ApiResponse::created(AccessKeyCreatedData {
        id: output.key.id().as_uuid(),
        name: output.key.name().to_owned(),
        key_identifier: output.key_identifier,
        display_suffix: output.display_suffix,
        secret: output.plaintext.expose_secret().to_owned(),
    }))
}

async fn list_access_keys(
    State(state): State<ApiState>,
    AuthenticatedUser { user_id }: AuthenticatedUser,
    Query(query): Query<ListAccessKeysQuery>,
) -> Result<ApiResponse<AccessKeyPageData>, ApiError> {
    let page = state
        .access_key_management
        .list(
            user_id,
            query.cursor.map(AccessKeyId::from_uuid),
            query.limit.unwrap_or(20),
        )
        .await?;
    let next_cursor = page
        .has_more
        .then(|| page.items.last().map(|item| item.id.as_uuid()))
        .flatten();
    let items = page
        .items
        .into_iter()
        .map(|item| AccessKeyData {
            id: item.id.as_uuid(),
            name: item.name,
            key_identifier: item.key_identifier,
            display_suffix: item.display_suffix,
            created_at: ApiDateTime::from(item.created_at),
            last_used_at: item.last_used_at.map(ApiDateTime::from),
            revoked_at: item.revoked_at.map(ApiDateTime::from),
        })
        .collect();
    Ok(ApiResponse::ok(AccessKeyPageData {
        items,
        has_more: page.has_more,
        next_cursor,
    }))
}

async fn revoke_access_key(
    State(state): State<ApiState>,
    AuthenticatedUser { user_id }: AuthenticatedUser,
    Path(access_key_id): Path<Uuid>,
) -> Result<ApiResponse<serde_json::Value>, ApiError> {
    state
        .access_key_management
        .revoke(user_id, AccessKeyId::from_uuid(access_key_id))
        .await?;
    Ok(ApiResponse::ok(serde_json::json!({ "id": access_key_id })))
}
