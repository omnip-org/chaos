use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    merchant::{
        ChangeSalesChannelStatusInput, ChangeStoreStatusInput, CreateSalesChannelInput,
        UpdateSalesChannelInput, UpdateStoreInput,
    },
    ports::{AdminActor, IdempotencyRequest, SalesChannelAdminItem, StoreAdminItem},
};
use chaos_domain::merchant::{SalesChannelId, StoreId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, StoreContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/stores/{store_id}", get(get_store).put(update_store))
        .route("/stores/{store_id}/activate", post(activate_store))
        .route("/stores/{store_id}/archive", post(archive_store))
        .route(
            "/stores/{store_id}/sales-channels",
            get(list_sales_channels).post(create_sales_channel),
        )
        .route(
            "/stores/{store_id}/sales-channels/{sales_channel_id}",
            get(get_sales_channel).put(update_sales_channel),
        )
        .route(
            "/stores/{store_id}/sales-channels/{sales_channel_id}/activate",
            post(activate_sales_channel),
        )
        .route(
            "/stores/{store_id}/sales-channels/{sales_channel_id}/archive",
            post(archive_sales_channel),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    store_id: Uuid,
}

#[derive(Deserialize)]
struct SalesChannelPath {
    store_id: Uuid,
    sales_channel_id: Uuid,
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

async fn get_store(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<StoreData>, ApiError> {
    let item = state
        .store_administration
        .get_store(AdminActor::Store(actor), StoreId::from_uuid(path.store_id))
        .await?;
    Ok(ApiResponse::ok(store_data(item)?))
}

async fn update_store(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<UpdateStoreBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    let idempotency = body_request(&headers, path.store_id, None, &body)?;
    let id = state
        .store_administration
        .update_store(UpdateStoreInput {
            actor: AdminActor::Store(actor),
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
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_store_status(state, headers, actor, path, true).await
}

async fn archive_store(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_store_status(state, headers, actor, path, false).await
}

async fn change_store_status(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::StoreActor,
    path: StorePath,
    activate: bool,
) -> Result<ApiResponse<MutationData>, ApiError> {
    let input = ChangeStoreStatusInput {
        actor: AdminActor::Store(actor),
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
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<SalesChannelData>>, ApiError> {
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
            AdminActor::Store(actor),
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
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<SalesChannelPath>,
) -> Result<ApiResponse<SalesChannelData>, ApiError> {
    let item = state
        .store_administration
        .get_sales_channel(
            AdminActor::Store(actor),
            StoreId::from_uuid(path.store_id),
            SalesChannelId::from_uuid(path.sales_channel_id),
        )
        .await?;
    Ok(ApiResponse::ok(channel_data(item)?))
}

async fn create_sales_channel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<SalesChannelBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    let idempotency = body_request(&headers, path.store_id, None, &body)?;
    let id = state
        .store_administration
        .create_sales_channel(CreateSalesChannelInput {
            actor: AdminActor::Store(actor),
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
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<SalesChannelPath>,
    ApiJson(body): ApiJson<SalesChannelBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    let idempotency = body_request(&headers, path.store_id, Some(path.sales_channel_id), &body)?;
    let id = state
        .store_administration
        .update_sales_channel(UpdateSalesChannelInput {
            actor: AdminActor::Store(actor),
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
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<SalesChannelPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_channel_status(state, headers, actor, path, true).await
}

async fn archive_sales_channel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<SalesChannelPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_channel_status(state, headers, actor, path, false).await
}

async fn change_channel_status(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::StoreActor,
    path: SalesChannelPath,
    activate: bool,
) -> Result<ApiResponse<MutationData>, ApiError> {
    let input = ChangeSalesChannelStatusInput {
        actor: AdminActor::Store(actor),
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

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode};
    use chaos_domain::identity::UserId;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    use super::*;

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
            "INSERT INTO merchant.stores (id, code, name, status) \
             VALUES ($1, $2, 'Store HTTP', 'inactive')",
        )
        .bind(store_id.as_uuid())
        .bind(format!("store-{}", &suffix[12..28]))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, role) in [(owner_id, "owner"), (support_id, "member")] {
            sqlx::query(
                "INSERT INTO merchant.store_memberships \
                 (store_id, user_id, role) \
                 VALUES ($1, $2, $3::merchant.store_role)",
            )
            .bind(store_id.as_uuid())
            .bind(id.as_uuid())
            .bind(role)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.store_currencies \
             (store_id, currency) VALUES ($1, 'USD')",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, 'web', 'Online Store', 'web', true)",
        )
        .bind(default_channel_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let store_uri = format!("/admin/v1/stores/{}", store_id.as_uuid());
        let channels_uri = format!("{store_uri}/sales-channels");
        let owner_state = test_state(&database_url, owner_id);

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &store_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["status"], "inactive");

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
}
