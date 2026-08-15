use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::merchant::{CreateMerchantAccountInput, CreateStoreInput};
use chaos_application::{ApplicationError, ports::IdempotencyRequest};
use chaos_domain::{
    FieldViolation,
    merchant::{MerchantAccountId, StoreId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiError, ApiJson, ApiQuery, ApiResponse, ApiState, AuthenticatedSession, MerchantContext,
    PageMeta, ResponseMeta,
};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts",
            post(create_merchant_account).get(list_merchant_accounts),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores",
            post(create_store).get(list_stores),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

const IDEMPOTENCY_KEY: &str = "idempotency-key";
const CURSOR_VERSION: u8 = 1;

#[derive(Clone, Copy)]
pub(super) enum CursorKind {
    MerchantAccount = 1,
    Store = 2,
    ApiKey = 3,
    Product = 4,
    StorefrontProduct = 5,
    PriceList = 6,
    SalesChannel = 7,
    InventoryLocation = 8,
    StockItem = 9,
}

#[derive(Deserialize, Serialize)]
struct CreateMerchantAccountBody {
    slug: String,
    display_name: String,
}

#[derive(Serialize)]
struct MerchantAccountCreatedData {
    id: Uuid,
}

#[derive(Serialize)]
struct MerchantAccountListData {
    id: Uuid,
    slug: String,
    display_name: String,
    status: &'static str,
    role: &'static str,
}

#[derive(Deserialize, Serialize)]
struct CreateStoreBody {
    code: String,
    name: String,
    #[serde(default = "default_region")]
    default_region: String,
    #[serde(default = "default_currency")]
    default_currency: String,
}

#[derive(Serialize)]
struct StoreCreatedData {
    id: Uuid,
}

#[derive(Serialize)]
struct StoreListData {
    id: Uuid,
    code: String,
    name: String,
    default_region: String,
    default_currency: String,
    status: &'static str,
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

async fn create_merchant_account(
    State(state): State<ApiState>,
    headers: HeaderMap,
    session: AuthenticatedSession,
    ApiJson(body): ApiJson<CreateMerchantAccountBody>,
) -> Result<ApiResponse<MerchantAccountCreatedData>, ApiError> {
    let idempotency_key = idempotency_key(&headers)?;
    let request_fingerprint = Sha256::digest(
        serde_json::to_vec(&body).map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .into();
    let output = state
        .create_merchant_account
        .execute(CreateMerchantAccountInput {
            owner_user_id: session.user_id,
            slug: body.slug,
            display_name: body.display_name,
            idempotency: IdempotencyRequest {
                key: idempotency_key,
                request_fingerprint,
            },
        })
        .await?;

    Ok(ApiResponse::created(MerchantAccountCreatedData {
        id: output.merchant_account_id.as_uuid(),
    }))
}

async fn list_merchant_accounts(
    State(state): State<ApiState>,
    session: AuthenticatedSession,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<MerchantAccountListData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::MerchantAccount))
        .transpose()?
        .map(MerchantAccountId::from_uuid);
    let page = state
        .merchant_queries
        .list_merchant_accounts(session.user_id, after, limit)
        .await?;
    let next_cursor = page
        .has_more
        .then(|| {
            page.items
                .last()
                .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::MerchantAccount))
        })
        .flatten();
    let data = page
        .items
        .into_iter()
        .map(|item| MerchantAccountListData {
            id: item.id.as_uuid(),
            slug: item.slug.as_str().into(),
            display_name: item.display_name,
            status: item.status.as_str(),
            role: item.role.as_str(),
        })
        .collect();
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor)))
}

async fn create_store(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiJson(body): ApiJson<CreateStoreBody>,
) -> Result<ApiResponse<StoreCreatedData>, ApiError> {
    let idempotency_key = idempotency_key(&headers)?;
    let request_fingerprint = Sha256::digest(
        serde_json::to_vec(&body).map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .into();
    let output = state
        .create_store
        .execute(CreateStoreInput {
            user_id: actor.user_id(),
            merchant_account_id: actor.merchant_account_id(),
            code: body.code,
            name: body.name,
            default_region: Some(body.default_region),
            default_currency: Some(body.default_currency),
            idempotency: IdempotencyRequest {
                key: idempotency_key,
                request_fingerprint,
            },
        })
        .await?;

    Ok(ApiResponse::created(StoreCreatedData {
        id: output.store_id.as_uuid(),
    }))
}

async fn list_stores(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<StoreListData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::Store))
        .transpose()?
        .map(StoreId::from_uuid);
    let page = state
        .merchant_queries
        .list_stores(actor, after, limit)
        .await?;
    let next_cursor = page
        .has_more
        .then(|| {
            page.items
                .last()
                .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::Store))
        })
        .flatten();
    let data = page
        .items
        .into_iter()
        .map(|item| StoreListData {
            id: item.id.as_uuid(),
            code: item.code.as_str().into(),
            name: item.name,
            default_region: item.default_region.as_str().into(),
            default_currency: item.default_currency.as_str().into(),
            status: item.status.as_str(),
        })
        .collect();
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor)))
}

pub(super) fn page_limit(limit: Option<u16>) -> Result<u16, ApiError> {
    match limit.unwrap_or(20) {
        limit @ 1..=100 => Ok(limit),
        _ => Err(ApplicationError::Validation {
            violations: vec![FieldViolation {
                field: "limit",
                reason: "must be between 1 and 100".into(),
            }],
        }
        .into()),
    }
}

pub(super) fn encode_cursor(id: Uuid, kind: CursorKind) -> String {
    let mut payload = [0_u8; 18];
    payload[0] = CURSOR_VERSION;
    payload[1] = kind as u8;
    payload[2..].copy_from_slice(id.as_bytes());
    URL_SAFE_NO_PAD.encode(payload)
}

pub(super) fn decode_cursor(cursor: &str, expected_kind: CursorKind) -> Result<Uuid, ApiError> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok();
    bytes
        .as_deref()
        .filter(|value| {
            value.len() == 18 && value[0] == CURSOR_VERSION && value[1] == expected_kind as u8
        })
        .and_then(|value| Uuid::from_slice(&value[2..]).ok())
        .ok_or_else(|| {
            ApplicationError::Validation {
                violations: vec![FieldViolation {
                    field: "cursor",
                    reason: "must be a valid opaque cursor".into(),
                }],
            }
            .into()
        })
}

pub(super) fn page_meta(has_more: bool, next_cursor: Option<String>) -> ResponseMeta {
    ResponseMeta {
        page: Some(PageMeta {
            has_more,
            next_cursor,
        }),
    }
}

fn default_region() -> String {
    "US".into()
}

fn default_currency() -> String {
    "USD".into()
}

pub(super) fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let key = headers
        .get(IDEMPOTENCY_KEY)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .map(str::to_owned)
        .ok_or_else(|| ApplicationError::Validation {
            violations: vec![FieldViolation {
                field: "idempotency_key",
                reason: "must be a non-empty Idempotency-Key header of at most 255 bytes".into(),
            }],
        })?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderValue, Method, StatusCode};
    use chaos_domain::identity::UserId;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    use super::*;

    #[test]
    fn accepts_a_bounded_idempotency_key() {
        let mut headers = HeaderMap::new();
        headers.insert(
            IDEMPOTENCY_KEY,
            HeaderValue::from_static("create-storefront"),
        );

        assert_eq!(idempotency_key(&headers).unwrap(), "create-storefront");
    }

    #[test]
    fn rejects_a_missing_idempotency_key() {
        assert!(idempotency_key(&HeaderMap::new()).is_err());
    }

    #[test]
    fn store_request_defaults_are_canonical_before_fingerprinting() {
        let omitted: CreateStoreBody =
            serde_json::from_str(r#"{"code":"primary","name":"Primary Store"}"#).unwrap();
        let explicit: CreateStoreBody = serde_json::from_str(
            r#"{"code":"primary","name":"Primary Store","default_region":"US","default_currency":"USD"}"#,
        )
        .unwrap();

        assert_eq!(omitted.default_region, "US");
        assert_eq!(omitted.default_currency, "USD");
        assert_eq!(
            serde_json::to_vec(&omitted).unwrap(),
            serde_json::to_vec(&explicit).unwrap()
        );
    }

    #[test]
    fn cursor_round_trips_without_exposing_uuid_text() {
        let id = Uuid::now_v7();
        let cursor = encode_cursor(id, CursorKind::MerchantAccount);

        assert_eq!(
            decode_cursor(&cursor, CursorKind::MerchantAccount).unwrap(),
            id
        );
        assert!(!cursor.contains(&id.to_string()));
        assert!(decode_cursor(&cursor, CursorKind::Store).is_err());
        assert!(decode_cursor("not-a-cursor", CursorKind::MerchantAccount).is_err());
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn merchant_and_store_provisioning_http_paths_are_end_to_end() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let owner_id = UserId::new();
        let support_id = UserId::new();
        let suffix = Uuid::now_v7().simple().to_string();
        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("merchant-http-{role}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        let owner_state = test_state(&database_url, owner_id);
        let account_body = json!({
            "slug": format!("merchant-http-{suffix}"),
            "display_name": "Merchant HTTP"
        });
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/merchant-accounts",
                Some(&format!("create-account-http-{suffix}")),
                Some(account_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let account_id = response_json(response).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::GET,
                "/admin/v1/merchant-accounts",
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let stores_uri = format!("/admin/v1/merchant-accounts/{account_id}/stores");
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &stores_uri,
                Some(&format!("create-store-http-{suffix}")),
                Some(json!({
                    "code": "primary-store",
                    "name": "Primary Store",
                    "default_region": "SG",
                    "default_currency": "SGD"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &stores_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"][0]["default_currency"], "SGD");

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/merchant-accounts",
                Some(&format!("invalid-account-http-{suffix}")),
                Some(json!({ "slug": "INVALID", "display_name": "Invalid" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                "/admin/v1/merchant-accounts",
                Some(&format!("conflict-account-http-{suffix}")),
                Some(account_body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let account_uuid = Uuid::parse_str(&account_id).unwrap();
        sqlx::query(
            "INSERT INTO merchant.merchant_account_memberships \
             (merchant_account_id, user_id, role) VALUES ($1, $2, 'support')",
        )
        .bind(account_uuid)
        .bind(support_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        let support_state = test_state(&database_url, support_id);
        let response = router(support_state)
            .oneshot(request(
                Method::POST,
                &stores_uri,
                Some(&format!("forbidden-store-http-{suffix}")),
                Some(json!({ "code": "support-store", "name": "No" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = router(owner_state)
            .oneshot(request(
                Method::POST,
                &format!("/admin/v1/merchant-accounts/{}/stores", Uuid::now_v7()),
                Some(&format!("unknown-account-http-{suffix}")),
                Some(json!({ "code": "unknown-store", "name": "No" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
