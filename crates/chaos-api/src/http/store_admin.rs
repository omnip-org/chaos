use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    merchant::{
        ArchiveStoreDomainInput, ChangeSalesChannelStatusInput, ChangeStoreStatusInput,
        CreateSalesChannelInput, CreateStoreDomainInput, UpdateSalesChannelInput, UpdateStoreInput,
        VerifyStoreDomainInput,
    },
    ports::{
        AdminActor, IdempotencyRequest, ResolvedStoreDomain, SalesChannelAdminItem, StoreAdminItem,
        StoreDomainItem,
    },
};
use chaos_domain::merchant::{MerchantAccountId, SalesChannelId, StoreDomainId, StoreId};
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
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}",
            get(get_store).put(update_store),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/activate",
            post(activate_store),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/archive",
            post(archive_store),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/sales-channels",
            get(list_sales_channels).post(create_sales_channel),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/sales-channels/{sales_channel_id}",
            get(get_sales_channel).put(update_sales_channel),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/sales-channels/{sales_channel_id}/activate",
            post(activate_sales_channel),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/sales-channels/{sales_channel_id}/archive",
            post(archive_sales_channel),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/domains",
            get(list_store_domains).post(create_store_domain),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/domains/{domain_id}/verify",
            post(verify_store_domain),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/domains/{domain_id}/archive",
            post(archive_store_domain),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

pub(super) fn domain_resolution_routes() -> Router<ApiState> {
    Router::new().route("/domain-context", get(resolve_store_domain))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
struct SalesChannelPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    sales_channel_id: Uuid,
}

#[derive(Deserialize)]
struct StoreDomainPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    domain_id: Uuid,
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UpdateStoreBody {
    code: String,
    name: String,
    default_region: String,
    default_currency: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SalesChannelBody {
    code: String,
    name: String,
    kind: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateStoreDomainBody {
    hostname: String,
    sales_channel_id: Uuid,
}

#[derive(Serialize)]
struct MutationData {
    id: Uuid,
}

#[derive(Serialize)]
struct StoreData {
    id: Uuid,
    code: String,
    name: String,
    default_region: String,
    default_currency: String,
    status: &'static str,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct SalesChannelData {
    id: Uuid,
    code: String,
    name: String,
    kind: &'static str,
    status: &'static str,
    is_default: bool,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct StoreDomainData {
    id: Uuid,
    store_id: Uuid,
    sales_channel_id: Uuid,
    hostname: String,
    domain_status: &'static str,
    created_by: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    verified_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    archived_at: Option<ApiDateTime>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct StoreDomainCreatedData {
    #[serde(flatten)]
    domain: StoreDomainData,
    verification_record_name: String,
    verification_record_value: String,
}

#[derive(Serialize)]
struct ResolvedStoreDomainData {
    store_id: Uuid,
    sales_channel_id: Uuid,
    hostname: String,
}

async fn get_store(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<StoreData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let item = state
        .store_administration
        .get_store(
            AdminActor::Merchant(actor),
            StoreId::from_uuid(path.store_id),
        )
        .await?;
    Ok(ApiResponse::ok(store_data(item)?))
}

async fn update_store(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<UpdateStoreBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = body_request(&headers, path.store_id, None, &body)?;
    let id = state
        .store_administration
        .update_store(UpdateStoreInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            code: body.code,
            name: body.name,
            default_region: body.default_region,
            default_currency: body.default_currency,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn activate_store(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_store_status(state, headers, actor, path, true).await
}

async fn archive_store(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_store_status(state, headers, actor, path, false).await
}

async fn change_store_status(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::MerchantActor,
    path: StorePath,
    activate: bool,
) -> Result<ApiResponse<MutationData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let input = ChangeStoreStatusInput {
        actor: AdminActor::Merchant(actor),
        store_id: StoreId::from_uuid(path.store_id),
        idempotency: action_request(&headers, path.store_id, None, activate)?,
    };
    let id = if activate {
        state.store_administration.activate_store(input).await?
    } else {
        state.store_administration.archive_store(input).await?
    };
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn list_sales_channels(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<SalesChannelData>>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::SalesChannel))
        .transpose()?
        .map(SalesChannelId::from_uuid);
    let page = state
        .store_administration
        .list_sales_channels(
            AdminActor::Merchant(actor),
            StoreId::from_uuid(path.store_id),
            after,
            limit,
        )
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::SalesChannel))
    });
    let data = page
        .items
        .into_iter()
        .map(channel_data)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor.flatten())))
}

async fn get_sales_channel(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<SalesChannelPath>,
) -> Result<ApiResponse<SalesChannelData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let item = state
        .store_administration
        .get_sales_channel(
            AdminActor::Merchant(actor),
            StoreId::from_uuid(path.store_id),
            SalesChannelId::from_uuid(path.sales_channel_id),
        )
        .await?;
    Ok(ApiResponse::ok(channel_data(item)?))
}

async fn create_sales_channel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<SalesChannelBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = body_request(&headers, path.store_id, None, &body)?;
    let id = state
        .store_administration
        .create_sales_channel(CreateSalesChannelInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            code: body.code,
            name: body.name,
            kind: body.kind,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(MutationData { id: id.as_uuid() }))
}

async fn update_sales_channel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<SalesChannelPath>,
    ApiJson(body): ApiJson<SalesChannelBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = body_request(&headers, path.store_id, Some(path.sales_channel_id), &body)?;
    let id = state
        .store_administration
        .update_sales_channel(UpdateSalesChannelInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            sales_channel_id: SalesChannelId::from_uuid(path.sales_channel_id),
            code: body.code,
            name: body.name,
            kind: body.kind,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn activate_sales_channel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<SalesChannelPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_channel_status(state, headers, actor, path, true).await
}

async fn archive_sales_channel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<SalesChannelPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_channel_status(state, headers, actor, path, false).await
}

async fn change_channel_status(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::MerchantActor,
    path: SalesChannelPath,
    activate: bool,
) -> Result<ApiResponse<MutationData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let input = ChangeSalesChannelStatusInput {
        actor: AdminActor::Merchant(actor),
        store_id: StoreId::from_uuid(path.store_id),
        sales_channel_id: SalesChannelId::from_uuid(path.sales_channel_id),
        idempotency: action_request(
            &headers,
            path.store_id,
            Some(path.sales_channel_id),
            activate,
        )?,
    };
    let id = if activate {
        state
            .store_administration
            .activate_sales_channel(input)
            .await?
    } else {
        state
            .store_administration
            .archive_sales_channel(input)
            .await?
    };
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn create_store_domain(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreateStoreDomainBody>,
) -> Result<ApiResponse<StoreDomainCreatedData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let idempotency = body_request(&headers, path.store_id, None, &body)?;
    let created = state
        .store_domain_administration
        .create(CreateStoreDomainInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            sales_channel_id: SalesChannelId::from_uuid(body.sales_channel_id),
            hostname: body.hostname,
            idempotency,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(StoreDomainCreatedData {
        domain: store_domain_data(created.domain),
        verification_record_name: created.verification_record_name,
        verification_record_value: created.verification_record_value.expose_secret().to_owned(),
    }))
}

async fn list_store_domains(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<StoreDomainData>>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::StoreDomain))
        .transpose()?
        .map(StoreDomainId::from_uuid);
    let page = state
        .store_domain_administration
        .list(actor, StoreId::from_uuid(path.store_id), after, limit)
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::StoreDomain))
    });
    Ok(
        ApiResponse::ok(page.items.into_iter().map(store_domain_data).collect())
            .with_meta(page_meta(page.has_more, next_cursor.flatten())),
    )
}

async fn verify_store_domain(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StoreDomainPath>,
) -> Result<ApiResponse<StoreDomainData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let item = state
        .store_domain_administration
        .verify(VerifyStoreDomainInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            domain_id: StoreDomainId::from_uuid(path.domain_id),
            idempotency: action_request(&headers, path.store_id, Some(path.domain_id), true)?,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(store_domain_data(item)))
}

async fn archive_store_domain(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StoreDomainPath>,
) -> Result<ApiResponse<StoreDomainData>, ApiError> {
    ensure_account(actor.merchant_account_id(), path.merchant_account_id)?;
    let item = state
        .store_domain_administration
        .archive(ArchiveStoreDomainInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            domain_id: StoreDomainId::from_uuid(path.domain_id),
            idempotency: action_request(&headers, path.store_id, Some(path.domain_id), false)?,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(store_domain_data(item)))
}

async fn resolve_store_domain(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<ApiResponse<ResolvedStoreDomainData>, ApiError> {
    let authority = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
        .ok_or_else(invalid_host)?;
    let item = state
        .store_domain_administration
        .resolve(authority.host().to_ascii_lowercase())
        .await?;
    Ok(ApiResponse::ok(resolved_store_domain_data(item)))
}

fn body_request<T: Serialize>(
    headers: &HeaderMap,
    store_id: Uuid,
    resource_id: Option<Uuid>,
    body: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(store_id, resource_id, body))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

fn action_request(
    headers: &HeaderMap,
    store_id: Uuid,
    resource_id: Option<Uuid>,
    activate: bool,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            format!("{store_id}:{resource_id:?}:{activate}").as_bytes(),
        )
        .into(),
    })
}

fn store_data(item: StoreAdminItem) -> Result<StoreData, ApplicationError> {
    Ok(StoreData {
        id: item.id.as_uuid(),
        code: item.code.as_str().into(),
        name: item.name,
        default_region: item.default_region.as_str().into(),
        default_currency: item.default_currency.as_str().into(),
        status: item.status.as_str(),
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    })
}

fn channel_data(item: SalesChannelAdminItem) -> Result<SalesChannelData, ApplicationError> {
    Ok(SalesChannelData {
        id: item.id.as_uuid(),
        code: item.code.as_str().into(),
        name: item.name,
        kind: item.kind.as_str(),
        status: item.status.as_str(),
        is_default: item.is_default,
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    })
}

fn store_domain_data(item: StoreDomainItem) -> StoreDomainData {
    StoreDomainData {
        id: item.id.as_uuid(),
        store_id: item.store_id.as_uuid(),
        sales_channel_id: item.sales_channel_id.as_uuid(),
        hostname: item.hostname.as_str().into(),
        domain_status: item.domain_status.as_str(),
        created_by: item.created_by.as_uuid(),
        verified_at: item.verified_at.map(Into::into),
        archived_at: item.archived_at.map(Into::into),
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    }
}

fn resolved_store_domain_data(item: ResolvedStoreDomain) -> ResolvedStoreDomainData {
    ResolvedStoreDomainData {
        store_id: item.store_id.as_uuid(),
        sales_channel_id: item.sales_channel_id.as_uuid(),
        hostname: item.hostname.as_str().into(),
    }
}

fn invalid_host() -> ApiError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "host",
            reason: "must be a valid canonical Store hostname".into(),
        }],
    }
    .into()
}

fn ensure_account(actual: MerchantAccountId, expected: Uuid) -> Result<(), ApiError> {
    if actual.as_uuid() == expected {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden.into())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use async_trait::async_trait;
    use axum::http::{Method, StatusCode};
    use chaos_application::{
        merchant::StoreDomainAdministration,
        ports::{
            StoreDomainOwnershipVerifier, StoreDomainVerificationMaterial,
            StoreDomainVerificationMaterialGenerator,
        },
    };
    use chaos_domain::{identity::UserId, merchant::StoreHostname};
    use chaos_infrastructure::repositories::PostgresStoreDomainRepository;
    use secrecy::SecretString;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    use super::*;

    struct FixedDomainMaterial;

    impl StoreDomainVerificationMaterialGenerator for FixedDomainMaterial {
        fn generate(&self) -> StoreDomainVerificationMaterial {
            StoreDomainVerificationMaterial {
                plaintext_token: SecretString::from("fixed-domain-proof"),
                token_digest: Sha256::digest(b"fixed-domain-proof").into(),
            }
        }
    }

    struct SwitchDomainVerification(Arc<AtomicBool>);

    #[async_trait]
    impl StoreDomainOwnershipVerifier for SwitchDomainVerification {
        async fn verify(
            &self,
            _hostname: &StoreHostname,
            expected_token_digest: &[u8; 32],
        ) -> Result<bool, ApplicationError> {
            Ok(self.0.load(Ordering::SeqCst)
                && *expected_token_digest
                    == <[u8; 32]>::from(Sha256::digest(b"fixed-domain-proof")))
        }
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn store_and_channel_http_matrix_covers_lifecycle_crud_and_errors() {
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
        let default_channel_id = SalesChannelId::new();
        let suffix = Uuid::now_v7().simple().to_string();

        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("store-http-{role}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Store HTTP Test')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("store-http-{suffix}"))
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
             VALUES ($1, $2, 'store-http', 'Store HTTP')",
        )
        .bind(store_id.as_uuid())
        .bind(account_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.store_currencies \
             (merchant_account_id, store_id, currency) VALUES ($1, $2, 'USD')",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, merchant_account_id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, $3, 'web', 'Online Store', 'web', true)",
        )
        .bind(default_channel_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let store_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let channels_uri = format!("{store_uri}/sales-channels");
        let owner_state = test_state(&database_url, owner_id);

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &store_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["status"], "draft");

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &store_uri,
                Some(&format!("update-store-http-{suffix}")),
                Some(json!({
                    "code": "store-http-updated",
                    "name": "Updated Store HTTP",
                    "default_region": "SG",
                    "default_currency": "SGD"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let shipping_uri = format!("{store_uri}/shipping-services");
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &shipping_uri,
                Some(&format!("create-shipping-http-{suffix}")),
                Some(json!({
                    "code": "standard",
                    "name": "Standard shipping",
                    "amount_minor": 700,
                    "currency": "SGD",
                    "estimated_min_days": 2,
                    "estimated_max_days": 5,
                    "destination_countries": ["sg", "MY"]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let shipping = response_json(response).await;
        let shipping_id = shipping["data"]["id"].as_str().unwrap();
        assert_eq!(
            shipping["data"]["destination_countries"],
            json!(["MY", "SG"])
        );

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &shipping_uri, None, None))
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

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &format!("{shipping_uri}/{shipping_id}/archive"),
                Some(&format!("archive-shipping-http-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["status"], "archived");

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &format!("{store_uri}/activate"),
                Some(&format!("activate-store-http-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &channels_uri,
                Some(&format!("create-channel-http-{suffix}")),
                Some(json!({ "code": "mobile", "name": "Mobile", "kind": "mobile" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let channel_id = response_json(response).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let channel_uri = format!("{channels_uri}/{channel_id}");

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &channels_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &channel_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &channel_uri,
                Some(&format!("update-channel-http-{suffix}")),
                Some(json!({
                    "code": "mobile-app",
                    "name": "Mobile App",
                    "kind": "mobile"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        for (action, key) in [
            ("archive", format!("archive-channel-http-{suffix}")),
            ("activate", format!("activate-channel-http-{suffix}")),
        ] {
            let response = router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &format!("{channel_uri}/{action}"),
                    Some(&key),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &format!("{channels_uri}/{}/archive", default_channel_id.as_uuid()),
                Some(&format!("archive-default-http-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &channels_uri,
                Some(&format!("invalid-channel-http-{suffix}")),
                Some(json!({ "code": "invalid", "name": "Invalid", "kind": "unknown" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &channels_uri,
                Some(&format!("conflict-channel-http-{suffix}")),
                Some(json!({ "code": "web", "name": "Conflict", "kind": "web" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "sales_channel_code_taken"
        );

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::GET,
                &format!("{channels_uri}/{}", Uuid::now_v7()),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let support_state = test_state(&database_url, support_id);
        let response = router(support_state)
            .oneshot(request(
                Method::POST,
                &channels_uri,
                Some(&format!("forbidden-channel-http-{suffix}")),
                Some(json!({ "code": "support", "name": "No", "kind": "custom" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = router(owner_state)
            .oneshot(request(
                Method::POST,
                &format!("{store_uri}/archive"),
                Some(&format!("archive-store-http-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn store_domain_http_requires_verification_and_never_broadens_store_context() {
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
        let web_channel_id = SalesChannelId::new();
        let suffix = Uuid::now_v7().simple().to_string();
        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1,$2)")
                .bind(id.as_uuid())
                .bind(format!("domain-{role}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1,$2,'Domain Test')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("domain-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query(
                "INSERT INTO merchant.merchant_account_memberships \
                 (merchant_account_id,user_id,role) VALUES ($1,$2,$3::merchant.merchant_role)",
            )
            .bind(account_id.as_uuid())
            .bind(id.as_uuid())
            .bind(role)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.stores \
             (id,merchant_account_id,code,name,status) VALUES ($1,$2,'domain','Domain','active')",
        )
        .bind(store_id.as_uuid())
        .bind(account_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id,merchant_account_id,store_id,code,name,kind,status,is_default) \
             VALUES ($1,$2,$3,'web','Web','web','active',true)",
        )
        .bind(web_channel_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let mut owner_state = test_state(&database_url, owner_id);
        let repository = Arc::new(PostgresStoreDomainRepository::new(
            owner_state.infrastructure.runtime_pool(),
        ));
        let verification_visible = Arc::new(AtomicBool::new(false));
        owner_state.store_domain_administration = Arc::new(StoreDomainAdministration::new(
            repository,
            Arc::new(FixedDomainMaterial),
            Arc::new(SwitchDomainVerification(verification_visible.clone())),
        ));
        let app = router(owner_state.clone());
        let hostname = format!("shop-{suffix}.merchant-commerce.com");
        let domains_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/domains",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let create_key = format!("create-domain-{suffix}");
        let create_body = json!({
            "hostname": hostname,
            "sales_channel_id": web_channel_id.as_uuid()
        });
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                &domains_uri,
                Some(&create_key),
                Some(create_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = response_json(response).await;
        let domain_id = created["data"]["id"].as_str().unwrap();
        assert_eq!(created["data"]["domain_status"], "pending");
        assert_eq!(
            created["data"]["verification_record_value"],
            "chaos-domain-verification=fixed-domain-proof"
        );

        let replay = app
            .clone()
            .oneshot(request(
                Method::POST,
                &domains_uri,
                Some(&create_key),
                Some(create_body),
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::CONFLICT);

        let mut resolve_request = request(Method::GET, "/store/v1/domain-context", None, None);
        resolve_request
            .headers_mut()
            .insert(axum::http::header::HOST, hostname.parse().unwrap());
        assert_eq!(
            app.clone().oneshot(resolve_request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );

        let domain_uri = format!("{domains_uri}/{domain_id}");
        let verify_key = format!("verify-domain-{suffix}");
        let pending = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{domain_uri}/verify"),
                Some(&verify_key),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(pending.status(), StatusCode::CONFLICT);
        verification_visible.store(true, Ordering::SeqCst);
        let verified = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{domain_uri}/verify"),
                Some(&verify_key),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(verified.status(), StatusCode::OK);
        assert_eq!(
            response_json(verified).await["data"]["domain_status"],
            "verified"
        );

        let mut resolve_request = request(Method::GET, "/store/v1/domain-context", None, None);
        resolve_request
            .headers_mut()
            .insert(axum::http::header::HOST, hostname.parse().unwrap());
        let resolved = app.clone().oneshot(resolve_request).await.unwrap();
        assert_eq!(resolved.status(), StatusCode::OK);
        let resolved = response_json(resolved).await;
        assert_eq!(resolved["data"]["store_id"], store_id.as_uuid().to_string());
        assert_eq!(
            resolved["data"]["sales_channel_id"],
            web_channel_id.as_uuid().to_string()
        );

        let listed = app
            .clone()
            .oneshot(request(Method::GET, &domains_uri, None, None))
            .await
            .unwrap();
        let listed = response_json(listed).await;
        assert!(listed["data"][0].get("verification_record_value").is_none());
        assert!(listed["data"][0].get("verification_token_digest").is_none());

        let runtime_pool = owner_state.infrastructure.runtime_pool();
        let mut isolated = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id',$1,true)")
            .bind(Uuid::now_v7().to_string())
            .execute(&mut *isolated)
            .await
            .unwrap();
        let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM merchant.store_domains")
            .fetch_one(&mut *isolated)
            .await
            .unwrap();
        assert_eq!(visible, 0);
        isolated.rollback().await.unwrap();

        let audit_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM merchant.store_domain_events WHERE store_domain_id = $1",
        )
        .bind(Uuid::parse_str(domain_id).unwrap())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(audit_events, 2);
        let mut immutable = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id',$1,true)")
            .bind(account_id.as_uuid().to_string())
            .execute(&mut *immutable)
            .await
            .unwrap();
        assert!(
            sqlx::query(
                "UPDATE merchant.store_domain_events SET event_kind = 'created' \
                 WHERE store_domain_id = $1",
            )
            .bind(Uuid::parse_str(domain_id).unwrap())
            .execute(&mut *immutable)
            .await
            .is_err()
        );
        immutable.rollback().await.unwrap();

        let archived = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("{domain_uri}/archive"),
                Some(&format!("archive-domain-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(archived.status(), StatusCode::OK);
        assert_eq!(
            response_json(archived).await["data"]["domain_status"],
            "archived"
        );
        let mut resolve_request = request(Method::GET, "/store/v1/domain-context", None, None);
        resolve_request
            .headers_mut()
            .insert(axum::http::header::HOST, hostname.parse().unwrap());
        assert_eq!(
            app.clone().oneshot(resolve_request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        let mut immutable = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id',$1,true)")
            .bind(account_id.as_uuid().to_string())
            .execute(&mut *immutable)
            .await
            .unwrap();
        assert!(
            sqlx::query("DELETE FROM merchant.store_domains WHERE id = $1")
                .bind(Uuid::parse_str(domain_id).unwrap())
                .execute(&mut *immutable)
                .await
                .is_err()
        );
        immutable.rollback().await.unwrap();

        let mut support_state = test_state(&database_url, support_id);
        support_state.store_domain_administration = owner_state.store_domain_administration;
        let forbidden = router(support_state)
            .oneshot(request(
                Method::POST,
                &domains_uri,
                Some(&format!("support-domain-{suffix}")),
                Some(json!({
                    "hostname": format!("support-{suffix}.merchant-commerce.com"),
                    "sales_channel_id": web_channel_id.as_uuid()
                })),
            ))
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    }
}
