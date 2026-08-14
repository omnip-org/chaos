use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, post},
};
use chaos_application::{ApplicationError, merchant::CreateApiKeyInput, ports::IdempotencyRequest};
use chaos_domain::merchant::{ApiKeyId, StoreId};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, MerchantContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
    response::format_time,
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/api-keys",
            post(create_api_key).get(list_api_keys),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/api-keys/{api_key_id}",
            delete(revoke_api_key),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
struct ApiKeyPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    api_key_id: Uuid,
}

#[derive(Deserialize, Serialize)]
struct CreateApiKeyBody {
    name: String,
    class: String,
    mode: String,
    scopes: Vec<String>,
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct ApiKeyCreatedData {
    id: Uuid,
    name: String,
    key_identifier: String,
    display_suffix: String,
    class: &'static str,
    mode: &'static str,
    scopes: Vec<&'static str>,
    secret: String,
}

#[derive(Serialize)]
struct ApiKeyListData {
    id: Uuid,
    name: String,
    key_identifier: String,
    display_suffix: String,
    class: &'static str,
    mode: &'static str,
    scopes: Vec<&'static str>,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    revoked_at: Option<String>,
}

async fn create_api_key(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreateApiKeyBody>,
) -> Result<ApiResponse<ApiKeyCreatedData>, ApiError> {
    ensure_account_path(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let idempotency_key = idempotency_key(&headers)?;
    let request_fingerprint = Sha256::digest(
        serde_json::to_vec(&(path.store_id, &body))
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .into();
    let output = state
        .api_key_management
        .create(CreateApiKeyInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            name: body.name,
            class: body.class,
            mode: body.mode,
            scopes: body.scopes,
            idempotency: IdempotencyRequest {
                key: idempotency_key,
                request_fingerprint,
            },
        })
        .await?;

    Ok(ApiResponse::created(ApiKeyCreatedData {
        id: output.api_key.id().as_uuid(),
        name: output.api_key.name().into(),
        key_identifier: output.key_identifier,
        display_suffix: output.display_suffix,
        class: output.api_key.class().as_str(),
        mode: output.api_key.mode().as_str(),
        scopes: output
            .api_key
            .scopes()
            .iter()
            .map(|scope| scope.as_str())
            .collect(),
        secret: output.plaintext.expose_secret().to_owned(),
    }))
}

async fn list_api_keys(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<ApiKeyListData>>, ApiError> {
    ensure_account_path(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::ApiKey))
        .transpose()?
        .map(ApiKeyId::from_uuid);
    let page = state
        .api_key_management
        .list(actor, StoreId::from_uuid(path.store_id), after, limit)
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::ApiKey))
    });
    let next_cursor = next_cursor.flatten();
    let data = page
        .items
        .into_iter()
        .map(|item| {
            Ok(ApiKeyListData {
                id: item.id.as_uuid(),
                name: item.name,
                key_identifier: item.key_identifier,
                display_suffix: item.display_suffix,
                class: item.class.as_str(),
                mode: item.mode.as_str(),
                scopes: item.scopes.iter().map(|scope| scope.as_str()).collect(),
                created_at: format_time(item.created_at)?,
                revoked_at: item.revoked_at.map(format_time).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;

    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor)))
}

async fn revoke_api_key(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ApiKeyPath>,
) -> Result<StatusCode, ApiError> {
    ensure_account_path(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let key = idempotency_key(&headers)?;
    let request_fingerprint =
        Sha256::digest(format!("{}:{}", path.store_id, path.api_key_id).as_bytes()).into();
    state
        .api_key_management
        .revoke(
            actor,
            StoreId::from_uuid(path.store_id),
            ApiKeyId::from_uuid(path.api_key_id),
            IdempotencyRequest {
                key,
                request_fingerprint,
            },
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn ensure_account_path(actual: Uuid, extracted: Uuid) -> Result<(), ApiError> {
    if actual == extracted {
        return Ok(());
    }
    Err(ApplicationError::Forbidden.into())
}

#[cfg(test)]
mod tests {
    use chaos_domain::merchant::{ApiKeyClass, ApiKeyMode};
    use chaos_infrastructure::repositories::SecureApiKeyMaterialGenerator;

    use super::*;
    use chaos_application::ports::ApiKeyMaterialGenerator;

    #[test]
    fn create_response_never_serializes_secret_material_by_accident() {
        let material =
            SecureApiKeyMaterialGenerator.generate(ApiKeyClass::Secret, ApiKeyMode::Test);
        let json = serde_json::to_value(ApiKeyCreatedData {
            id: Uuid::now_v7(),
            name: "MCP test".into(),
            key_identifier: material.key_identifier,
            display_suffix: material.display_suffix,
            class: "secret",
            mode: "test",
            scopes: vec!["mcp:tools"],
            secret: material.plaintext.expose_secret().to_owned(),
        })
        .unwrap();

        assert!(
            json["secret"]
                .as_str()
                .unwrap()
                .starts_with("cc_v1_test_secret_")
        );
        assert!(json.get("secret_digest").is_none());
    }
}
