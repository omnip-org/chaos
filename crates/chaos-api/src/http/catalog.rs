use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post, put},
};
use chaos_application::{
    ApplicationError,
    catalog::{
        ChangeProductStatusInput, CreateProductInput, CreateProductOptionInput,
        CreateProductSelectedOptionInput, CreateProductVariantInput, ProductPublicationInput,
        UpdateProductInput,
    },
    ports::{AdminActor, IdempotencyRequest},
};
use chaos_domain::{
    catalog::ProductId,
    merchant::{SalesChannelId, StoreId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, StoreContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/stores/{store_id}/products",
            post(create_product).get(list_products),
        )
        .route(
            "/stores/{store_id}/products/{product_id}",
            get(get_product).put(update_product),
        )
        .route(
            "/stores/{store_id}/products/{product_id}/activate",
            post(activate_product),
        )
        .route(
            "/stores/{store_id}/products/{product_id}/archive",
            post(archive_product),
        )
        .route(
            "/stores/{store_id}/products/{product_id}/publications/{sales_channel_id}",
            put(publish_product).delete(unpublish_product),
        )
        .layer(DefaultBodyLimit::max(256 * 1024))
}

#[derive(Deserialize)]
struct ProductPath {
    store_id: Uuid,
}

#[derive(Deserialize)]
struct ProductDetailPath {
    store_id: Uuid,
    product_id: Uuid,
}

#[derive(Deserialize)]
struct ProductPublicationPath {
    store_id: Uuid,
    product_id: Uuid,
    sales_channel_id: Uuid,
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateProductBody {
    handle: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    options: Vec<CreateProductOptionBody>,
    #[serde(default)]
    variants: Vec<CreateProductVariantBody>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UpdateProductBody {
    handle: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateProductOptionBody {
    name: String,
    values: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateProductVariantBody {
    title: String,
    sku: Option<String>,
    #[serde(default = "enabled")]
    requires_shipping: bool,
    #[serde(default = "enabled")]
    track_inventory: bool,
    #[serde(default)]
    selected_options: Vec<CreateProductSelectedOptionBody>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateProductSelectedOptionBody {
    option: String,
    value: String,
}

#[derive(Serialize)]
struct ProductCreatedData {
    id: Uuid,
}

#[derive(Serialize)]
struct ProductMutationData {
    id: Uuid,
}

#[derive(Serialize)]
struct ProductListData {
    id: Uuid,
    handle: String,
    title: String,
    status: &'static str,
    variant_count: u32,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct ProductOptionValueData {
    id: Uuid,
    value: String,
    position: u16,
}

#[derive(Serialize)]
struct ProductOptionData {
    id: Uuid,
    name: String,
    position: u16,
    values: Vec<ProductOptionValueData>,
}

#[derive(Serialize)]
struct SelectedOptionData {
    option_id: Uuid,
    option_name: String,
    option_value_id: Uuid,
    value: String,
}

#[derive(Serialize)]
struct ProductVariantData {
    id: Uuid,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    status: &'static str,
    requires_shipping: bool,
    track_inventory: bool,
    selected_options: Vec<SelectedOptionData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct ProductDetailData {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    status: &'static str,
    options: Vec<ProductOptionData>,
    variants: Vec<ProductVariantData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

async fn create_product(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductPath>,
    ApiJson(body): ApiJson<CreateProductBody>,
) -> Result<ApiResponse<ProductCreatedData>, ApiError> {
    let idempotency_key = idempotency_key(&headers)?;
    let request_fingerprint = Sha256::digest(
        serde_json::to_vec(&(path.store_id, &body))
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .into();
    let output = state
        .create_product
        .execute(CreateProductInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            handle: body.handle,
            title: body.title,
            description: body.description,
            options: body
                .options
                .into_iter()
                .map(|option| CreateProductOptionInput {
                    name: option.name,
                    values: option.values,
                })
                .collect(),
            variants: body
                .variants
                .into_iter()
                .map(|variant| CreateProductVariantInput {
                    title: variant.title,
                    sku: variant.sku,
                    requires_shipping: variant.requires_shipping,
                    track_inventory: variant.track_inventory,
                    selected_options: variant
                        .selected_options
                        .into_iter()
                        .map(|selection| CreateProductSelectedOptionInput {
                            option: selection.option,
                            value: selection.value,
                        })
                        .collect(),
                    metadata: variant.metadata,
                })
                .collect(),
            metadata: body.metadata,
            idempotency: IdempotencyRequest {
                key: idempotency_key,
                request_fingerprint,
            },
        })
        .await?;
    Ok(ApiResponse::created(ProductCreatedData {
        id: output.product_id.as_uuid(),
    }))
}

async fn list_products(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductPath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<ProductListData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::Product))
        .transpose()?
        .map(ProductId::from_uuid);
    let page = state
        .catalog_queries
        .list_products(
            AdminActor::Store(actor),
            StoreId::from_uuid(path.store_id),
            after,
            limit,
        )
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::Product))
    });
    let data = page
        .items
        .into_iter()
        .map(|item| {
            Ok(ProductListData {
                id: item.id.as_uuid(),
                handle: item.handle,
                title: item.title,
                status: item.status.as_str(),
                variant_count: item.variant_count,
                created_at: item.created_at.into(),
                updated_at: item.updated_at.into(),
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor.flatten())))
}

async fn get_product(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductDetailPath>,
) -> Result<ApiResponse<ProductDetailData>, ApiError> {
    let product = state
        .catalog_queries
        .get_product(
            AdminActor::Store(actor),
            StoreId::from_uuid(path.store_id),
            ProductId::from_uuid(path.product_id),
        )
        .await?;
    let options = product
        .options
        .into_iter()
        .map(|option| ProductOptionData {
            id: option.id.as_uuid(),
            name: option.name,
            position: option.position,
            values: option
                .values
                .into_iter()
                .map(|value| ProductOptionValueData {
                    id: value.id.as_uuid(),
                    value: value.value,
                    position: value.position,
                })
                .collect(),
        })
        .collect();
    let variants = product
        .variants
        .into_iter()
        .map(|variant| {
            Ok(ProductVariantData {
                id: variant.id.as_uuid(),
                title: variant.title,
                sku: variant.sku,
                status: variant.status.as_str(),
                requires_shipping: variant.requires_shipping,
                track_inventory: variant.track_inventory,
                selected_options: variant
                    .selected_options
                    .into_iter()
                    .map(|selection| SelectedOptionData {
                        option_id: selection.option_id.as_uuid(),
                        option_name: selection.option_name,
                        option_value_id: selection.option_value_id.as_uuid(),
                        value: selection.value,
                    })
                    .collect(),
                metadata: variant.metadata,
                created_at: variant.created_at.into(),
                updated_at: variant.updated_at.into(),
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    Ok(ApiResponse::ok(ProductDetailData {
        id: product.id.as_uuid(),
        handle: product.handle,
        title: product.title,
        description: product.description,
        status: product.status.as_str(),
        options,
        variants,
        metadata: product.metadata,
        created_at: product.created_at.into(),
        updated_at: product.updated_at.into(),
    }))
}

async fn update_product(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductDetailPath>,
    ApiJson(body): ApiJson<UpdateProductBody>,
) -> Result<ApiResponse<ProductMutationData>, ApiError> {
    let request = mutation_request(
        &headers,
        serde_json::to_vec(&(path.store_id, path.product_id, &body))
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )?;
    let id = state
        .catalog_management
        .update(UpdateProductInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            product_id: ProductId::from_uuid(path.product_id),
            handle: body.handle,
            title: body.title,
            description: body.description,
            metadata: body.metadata,
            idempotency: request,
        })
        .await?;
    Ok(ApiResponse::ok(ProductMutationData { id: id.as_uuid() }))
}

async fn activate_product(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductDetailPath>,
) -> Result<ApiResponse<ProductMutationData>, ApiError> {
    change_product_status(state, headers, actor, path, true).await
}

async fn archive_product(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductDetailPath>,
) -> Result<ApiResponse<ProductMutationData>, ApiError> {
    change_product_status(state, headers, actor, path, false).await
}

async fn change_product_status(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::StoreActor,
    path: ProductDetailPath,
    activate: bool,
) -> Result<ApiResponse<ProductMutationData>, ApiError> {
    let action = if activate { "activate" } else { "archive" };
    let request = mutation_request(
        &headers,
        format!("{}:{}:{action}", path.store_id, path.product_id).into_bytes(),
    )?;
    let input = ChangeProductStatusInput {
        actor: AdminActor::Store(actor),
        store_id: StoreId::from_uuid(path.store_id),
        product_id: ProductId::from_uuid(path.product_id),
        idempotency: request,
    };
    let id = if activate {
        state.catalog_management.activate(input).await?
    } else {
        state.catalog_management.archive(input).await?
    };
    Ok(ApiResponse::ok(ProductMutationData { id: id.as_uuid() }))
}

async fn publish_product(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductPublicationPath>,
) -> Result<ApiResponse<ProductMutationData>, ApiError> {
    change_publication(state, headers, actor, path, true).await
}

async fn unpublish_product(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductPublicationPath>,
) -> Result<ApiResponse<ProductMutationData>, ApiError> {
    change_publication(state, headers, actor, path, false).await
}

async fn change_publication(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::StoreActor,
    path: ProductPublicationPath,
    publish: bool,
) -> Result<ApiResponse<ProductMutationData>, ApiError> {
    let action = if publish { "publish" } else { "unpublish" };
    let request = mutation_request(
        &headers,
        format!(
            "{}:{}:{}:{action}",
            path.store_id, path.product_id, path.sales_channel_id
        )
        .into_bytes(),
    )?;
    let input = ProductPublicationInput {
        actor: AdminActor::Store(actor),
        store_id: StoreId::from_uuid(path.store_id),
        product_id: ProductId::from_uuid(path.product_id),
        sales_channel_id: SalesChannelId::from_uuid(path.sales_channel_id),
        idempotency: request,
    };
    let id = if publish {
        state.catalog_management.publish(input).await?
    } else {
        state.catalog_management.unpublish(input).await?
    };
    Ok(ApiResponse::ok(ProductMutationData { id: id.as_uuid() }))
}

fn mutation_request(
    headers: &HeaderMap,
    fingerprint_source: Vec<u8>,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(fingerprint_source).into(),
    })
}

const fn enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
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
    async fn product_http_matrix_covers_crud_lifecycle_publication_and_errors() {
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
        let channel_id = SalesChannelId::new();
        let suffix = Uuid::now_v7().simple().to_string();

        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("catalog-http-{role}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.stores (id, code, name, status) \
             VALUES ($1, $2, 'Catalog HTTP', 'active')",
        )
        .bind(store_id.as_uuid())
        .bind(format!("catalog-{}", &suffix[12..28]))
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
            "INSERT INTO merchant.sales_channels \
             (id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, 'web', 'Web', 'web', true)",
        )
        .bind(channel_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let collection_uri = format!("/admin/v1/stores/{}/products", store_id.as_uuid());
        let product_body = json!({
            "handle": "http-shirt",
            "title": "HTTP Shirt",
            "description": "Created through the real HTTP router",
            "variants": [{
                "title": "Default",
                "sku": "HTTP-SHIRT",
                "requires_shipping": true,
                "track_inventory": true
            }]
        });
        let owner_state = test_state(&database_url, owner_id);
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &collection_uri,
                Some(&format!("create-product-{suffix}")),
                Some(product_body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let product_id = response_json(response).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let detail_uri = format!("{collection_uri}/{product_id}");

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &collection_uri, None, None))
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
            .oneshot(request(Method::GET, &detail_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["handle"],
            "http-shirt"
        );

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &detail_uri,
                Some(&format!("update-product-{suffix}")),
                Some(json!({
                    "handle": "http-shirt-updated",
                    "title": "Updated HTTP Shirt",
                    "description": "Updated"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        for (action, key) in [
            ("activate", format!("activate-product-{suffix}")),
            ("archive", format!("archive-product-{suffix}")),
        ] {
            let response = router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &format!("{detail_uri}/{action}"),
                    Some(&key),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            if action == "activate" {
                let publication_uri = format!("{detail_uri}/publications/{}", channel_id.as_uuid());
                for (method, key) in [
                    (Method::PUT, format!("publish-product-{suffix}")),
                    (Method::DELETE, format!("unpublish-product-{suffix}")),
                ] {
                    let response = router(owner_state.clone())
                        .oneshot(request(method, &publication_uri, Some(&key), None))
                        .await
                        .unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                }
            }
        }

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &collection_uri,
                Some(&format!("invalid-product-{suffix}")),
                Some(json!({ "handle": "INVALID", "title": "Invalid" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &collection_uri,
                Some(&format!("conflict-product-{suffix}")),
                Some(json!({ "handle": "http-shirt-updated", "title": "Conflict" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "product_handle_taken"
        );

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::GET,
                &format!("{collection_uri}/{}", Uuid::now_v7()),
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
                &collection_uri,
                Some(&format!("forbidden-product-{suffix}")),
                Some(json!({ "handle": "support-product", "title": "No" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let unauthenticated = router(test_state(&database_url, owner_id))
            .oneshot(
                Request::builder()
                    .uri(&collection_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    }
}
