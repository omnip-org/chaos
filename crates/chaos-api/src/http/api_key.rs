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
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, MerchantContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
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
    created_at: ApiDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    revoked_at: Option<ApiDateTime>,
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
                created_at: item.created_at.into(),
                revoked_at: item.revoked_at.map(Into::into),
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
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use chaos_domain::merchant::{ApiKeyClass, ApiKeyMode};
    use chaos_domain::{identity::UserId, merchant::MerchantAccountId};
    use chaos_infrastructure::repositories::SecureApiKeyMaterialGenerator;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;
    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };
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

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn api_key_http_lifecycle_preserves_one_time_secrets_and_immediate_revocation() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let owner_id = UserId::new();
        let support_id = UserId::new();
        let account_id = MerchantAccountId::new();
        let store_id = StoreId::new();
        let channel_id = chaos_domain::merchant::SalesChannelId::new();
        let suffix = Uuid::now_v7().simple().to_string();

        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("api-key-http-{role}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'API Key HTTP Test')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("api-key-http-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query(
                "INSERT INTO merchant.merchant_account_memberships \
                 (merchant_account_id, user_id, role) \
                 VALUES ($1, $2, $3::merchant.merchant_role)",
            )
            .bind(account_id.as_uuid())
            .bind(id.as_uuid())
            .bind(role)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.stores (id, merchant_account_id, code, name) \
             VALUES ($1, $2, 'api-key-http', 'API Key HTTP')",
        )
        .bind(store_id.as_uuid())
        .bind(account_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, merchant_account_id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, $3, 'web', 'Online Store', 'web', true)",
        )
        .bind(channel_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let keys_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/api-keys",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let key_body = json!({
            "name": "Browser test",
            "class": "publishable",
            "mode": "test",
            "scopes": ["catalog:read"]
        });
        let owner_state = test_state(&database_url, owner_id);
        let creation_key = format!("create-api-key-http-{suffix}");
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &keys_uri,
                Some(&creation_key),
                Some(key_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = response_json(response).await;
        let api_key_id = created["data"]["id"].as_str().unwrap().to_owned();
        let secret = created["data"]["secret"].as_str().unwrap().to_owned();
        assert!(secret.starts_with("cc_v1_test_publishable_"));

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &keys_uri,
                Some(&creation_key),
                Some(key_body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "api_key_secret_already_issued"
        );

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &keys_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let listed = response_json(response).await;
        assert!(listed["data"][0].get("secret").is_none());

        let response = router(owner_state.clone())
            .oneshot(
                Request::get("/store/v1/products")
                    .header("authorization", format!("Bearer {secret}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let key_uri = format!("{keys_uri}/{api_key_id}");
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::DELETE,
                &key_uri,
                Some(&format!("revoke-api-key-http-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let response = router(owner_state.clone())
            .oneshot(
                Request::get("/store/v1/products")
                    .header("authorization", format!("Bearer {secret}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &keys_uri,
                Some(&format!("invalid-api-key-http-{suffix}")),
                Some(json!({
                    "name": "Invalid",
                    "class": "publishable",
                    "mode": "test",
                    "scopes": ["orders:read"]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(owner_state)
            .oneshot(request(
                Method::DELETE,
                &format!("{keys_uri}/{}", Uuid::now_v7()),
                Some(&format!("missing-api-key-http-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let support_state = test_state(&database_url, support_id);
        let response = router(support_state)
            .oneshot(request(
                Method::POST,
                &keys_uri,
                Some(&format!("forbidden-api-key-http-{suffix}")),
                Some(json!({
                    "name": "Support",
                    "class": "secret",
                    "mode": "test",
                    "scopes": ["mcp:tools"]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
