use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post, put},
};
use chaos_application::{
    ApplicationError,
    catalog::{
        ChangeCollectionStatusInput, CollectionPublicationInput, CreateCollectionInput,
        ReplaceCollectionProductsInput, UpdateCollectionInput,
    },
    ports::{AdminActor, CollectionDetail, IdempotencyRequest, StorefrontCollectionItem},
};
use chaos_domain::{
    catalog::{CollectionId, ProductId},
    merchant::{MerchantAccountId, SalesChannelId, StoreId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, MerchantContext,
    StorefrontMachine,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
};

pub fn admin_routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/collections",
            post(create_collection).get(list_collections),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/collections/{collection_id}",
            get(get_collection).put(update_collection),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/collections/{collection_id}/activate",
            post(activate_collection),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/collections/{collection_id}/archive",
            post(archive_collection),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/collections/{collection_id}/products",
            put(replace_products),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/collections/{collection_id}/publications/{sales_channel_id}",
            put(publish_collection).delete(unpublish_collection),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}

pub fn storefront_routes() -> Router<ApiState> {
    Router::new()
        .route("/collections", get(list_storefront_collections))
        .route("/collections/{handle}", get(get_storefront_collection))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}
#[derive(Deserialize)]
struct DetailPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    collection_id: Uuid,
}
#[derive(Deserialize)]
struct PublicationPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    collection_id: Uuid,
    sales_channel_id: Uuid,
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CollectionBody {
    handle: String,
    title: String,
    #[serde(default)]
    description: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProductsBody {
    product_ids: Vec<Uuid>,
}

#[derive(Serialize)]
struct MutationData {
    id: Uuid,
}
#[derive(Serialize)]
struct CollectionListData {
    id: Uuid,
    handle: String,
    title: String,
    status: &'static str,
    product_count: u32,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}
#[derive(Serialize)]
struct CollectionProductData {
    product_id: Uuid,
    position: u32,
}
#[derive(Serialize)]
struct CollectionDetailData {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    status: &'static str,
    products: Vec<CollectionProductData>,
    published_sales_channel_ids: Vec<Uuid>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}
#[derive(Serialize)]
struct StorefrontCollectionData {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    product_count: u32,
    locale: String,
}

async fn create_collection(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CollectionBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let request = mutation(&headers, &(path.store_id, &body))?;
    let id = state
        .collection_administration
        .create(CreateCollectionInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            handle: body.handle,
            title: body.title,
            description: body.description,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(MutationData { id: id.as_uuid() }))
}

async fn list_collections(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<CollectionListData>>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|v| decode_cursor(v, CursorKind::Collection))
        .transpose()?
        .map(CollectionId::from_uuid);
    let page = state
        .collection_administration
        .list(
            AdminActor::Merchant(actor),
            StoreId::from_uuid(path.store_id),
            after,
            limit,
        )
        .await?;
    let next = page
        .has_more
        .then(|| {
            page.items
                .last()
                .map(|v| encode_cursor(v.id.as_uuid(), CursorKind::Collection))
        })
        .flatten();
    Ok(ApiResponse::ok(
        page.items
            .into_iter()
            .map(|v| CollectionListData {
                id: v.id.as_uuid(),
                handle: v.handle,
                title: v.title,
                status: v.status.as_str(),
                product_count: v.product_count,
                created_at: v.created_at.into(),
                updated_at: v.updated_at.into(),
            })
            .collect(),
    )
    .with_meta(page_meta(page.has_more, next)))
}

async fn get_collection(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<DetailPath>,
) -> Result<ApiResponse<CollectionDetailData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let item = state
        .collection_administration
        .get(
            AdminActor::Merchant(actor),
            StoreId::from_uuid(path.store_id),
            CollectionId::from_uuid(path.collection_id),
        )
        .await?;
    Ok(ApiResponse::ok(detail_data(item)))
}

async fn update_collection(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<DetailPath>,
    ApiJson(body): ApiJson<CollectionBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let request = mutation(&headers, &(path.store_id, path.collection_id, &body))?;
    let id = state
        .collection_administration
        .update(UpdateCollectionInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            collection_id: CollectionId::from_uuid(path.collection_id),
            handle: body.handle,
            title: body.title,
            description: body.description,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn activate_collection(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<DetailPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_status(state, headers, actor, path, true).await
}
async fn archive_collection(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<DetailPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    change_status(state, headers, actor, path, false).await
}
async fn change_status(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::MerchantActor,
    path: DetailPath,
    active: bool,
) -> Result<ApiResponse<MutationData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let input = ChangeCollectionStatusInput {
        actor: AdminActor::Merchant(actor),
        store_id: StoreId::from_uuid(path.store_id),
        collection_id: CollectionId::from_uuid(path.collection_id),
        idempotency: mutation(&headers, &(path.store_id, path.collection_id, active))?,
        now: state.clock.now(),
    };
    let id = if active {
        state.collection_administration.activate(input).await?
    } else {
        state.collection_administration.archive(input).await?
    };
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn replace_products(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<DetailPath>,
    ApiJson(body): ApiJson<ProductsBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let request = mutation(&headers, &(path.store_id, path.collection_id, &body))?;
    let id = state
        .collection_administration
        .replace_products(ReplaceCollectionProductsInput {
            actor: AdminActor::Merchant(actor),
            store_id: StoreId::from_uuid(path.store_id),
            collection_id: CollectionId::from_uuid(path.collection_id),
            product_ids: body
                .product_ids
                .into_iter()
                .map(ProductId::from_uuid)
                .collect(),
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn publish_collection(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<PublicationPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    publication(state, headers, actor, path, true).await
}
async fn unpublish_collection(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<PublicationPath>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    publication(state, headers, actor, path, false).await
}
async fn publication(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::MerchantActor,
    path: PublicationPath,
    published: bool,
) -> Result<ApiResponse<MutationData>, ApiError> {
    account(actor.merchant_account_id(), path.merchant_account_id)?;
    let input = CollectionPublicationInput {
        actor: AdminActor::Merchant(actor),
        store_id: StoreId::from_uuid(path.store_id),
        collection_id: CollectionId::from_uuid(path.collection_id),
        sales_channel_id: SalesChannelId::from_uuid(path.sales_channel_id),
        idempotency: mutation(
            &headers,
            &(
                path.store_id,
                path.collection_id,
                path.sales_channel_id,
                published,
            ),
        )?,
        now: state.clock.now(),
    };
    let id = if published {
        state.collection_administration.publish(input).await?
    } else {
        state.collection_administration.unpublish(input).await?
    };
    Ok(ApiResponse::ok(MutationData { id: id.as_uuid() }))
}

async fn list_storefront_collections(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<StorefrontCollectionData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|v| decode_cursor(v, CursorKind::StorefrontCollection))
        .transpose()?
        .map(CollectionId::from_uuid);
    let page = state
        .storefront_collections
        .list(&actor, query.locale.as_deref(), after, limit)
        .await?;
    let next = page
        .has_more
        .then(|| {
            page.items
                .last()
                .map(|v| encode_cursor(v.id.as_uuid(), CursorKind::StorefrontCollection))
        })
        .flatten();
    Ok(
        ApiResponse::ok(page.items.into_iter().map(storefront_data).collect())
            .with_meta(page_meta(page.has_more, next)),
    )
}
async fn get_storefront_collection(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiPath(path): ApiPath<HandlePath>,
    ApiQuery(query): ApiQuery<LocaleQuery>,
) -> Result<ApiResponse<StorefrontCollectionData>, ApiError> {
    Ok(ApiResponse::ok(storefront_data(
        state
            .storefront_collections
            .get(&actor, query.locale.as_deref(), &path.handle)
            .await?,
    )))
}

fn detail_data(v: CollectionDetail) -> CollectionDetailData {
    CollectionDetailData {
        id: v.id.as_uuid(),
        handle: v.handle,
        title: v.title,
        description: v.description,
        status: v.status.as_str(),
        products: v
            .products
            .into_iter()
            .map(|p| CollectionProductData {
                product_id: p.product_id.as_uuid(),
                position: p.position,
            })
            .collect(),
        published_sales_channel_ids: v
            .published_sales_channel_ids
            .into_iter()
            .map(SalesChannelId::as_uuid)
            .collect(),
        created_at: v.created_at.into(),
        updated_at: v.updated_at.into(),
    }
}
fn storefront_data(v: StorefrontCollectionItem) -> StorefrontCollectionData {
    StorefrontCollectionData {
        id: v.id.as_uuid(),
        handle: v.handle,
        title: v.title,
        description: v.description,
        product_count: v.product_count,
        locale: v.locale.as_str().into(),
    }
}
fn account(actual: MerchantAccountId, path: Uuid) -> Result<(), ApiError> {
    if actual.as_uuid() == path {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden.into())
    }
}
fn mutation<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(value).map_err(|e| ApplicationError::Unexpected(e.into()))?,
        )
        .into(),
    })
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use chaos_application::ports::{ApiKeyMaterialGenerator, GeneratedApiKeyMaterial};
    use chaos_domain::{
        identity::UserId,
        merchant::{ApiKeyClass, ApiKeyId, ApiKeyMode},
    };
    use chaos_infrastructure::repositories::SecureApiKeyMaterialGenerator;
    use secrecy::ExposeSecret;
    use serde_json::json;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    use super::*;

    async fn publishable_key(
        pool: &PgPool,
        account_id: MerchantAccountId,
        store_id: StoreId,
        channel_id: SalesChannelId,
        user_id: UserId,
    ) -> GeneratedApiKeyMaterial {
        let material =
            SecureApiKeyMaterialGenerator.generate(ApiKeyClass::Publishable, ApiKeyMode::Live);
        let key_id = ApiKeyId::new();
        sqlx::query("INSERT INTO merchant.api_keys (id,merchant_account_id,store_id,sales_channel_id,key_identifier,secret_digest,display_suffix,name,class,mode,created_by_user_id) VALUES ($1,$2,$3,$4,$5,$6,$7,'Collection Storefront','publishable','live',$8)")
            .bind(key_id.as_uuid()).bind(account_id.as_uuid()).bind(store_id.as_uuid()).bind(channel_id.as_uuid()).bind(&material.key_identifier).bind(material.secret_digest.as_slice()).bind(&material.display_suffix).bind(user_id.as_uuid()).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO merchant.api_key_scopes (merchant_account_id,api_key_id,scope) VALUES ($1,$2,'catalog:read')")
            .bind(account_id.as_uuid()).bind(key_id.as_uuid()).execute(pool).await.unwrap();
        material
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn collection_http_enforces_lifecycle_publication_and_isolation() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let owner = UserId::new();
        let support = UserId::new();
        let account = MerchantAccountId::new();
        let other_account = MerchantAccountId::new();
        let store = StoreId::new();
        let other_store = StoreId::new();
        let channel = SalesChannelId::new();
        let first = ProductId::new();
        let second = ProductId::new();
        let foreign = ProductId::new();
        let first_variant = Uuid::now_v7();
        let second_variant = Uuid::now_v7();
        let price_list = Uuid::now_v7();
        let suffix = Uuid::now_v7().simple().to_string();

        for (id, label) in [(owner, "owner"), (support, "support")] {
            sqlx::query("INSERT INTO identity.users (id,email) VALUES ($1,$2)")
                .bind(id.as_uuid())
                .bind(format!("collection-{label}-{suffix}@example.com"))
                .execute(&pool)
                .await
                .unwrap();
        }
        for (id, slug) in [
            (account, format!("collection-{suffix}")),
            (other_account, format!("collection-other-{suffix}")),
        ] {
            sqlx::query("INSERT INTO merchant.merchant_accounts (id,slug,display_name) VALUES ($1,$2,'Collection Test')").bind(id.as_uuid()).bind(slug).execute(&pool).await.unwrap();
        }
        for (id, role) in [(owner, "owner"), (support, "support")] {
            sqlx::query("INSERT INTO merchant.merchant_account_memberships (merchant_account_id,user_id,role) VALUES ($1,$2,$3::merchant.merchant_role)").bind(account.as_uuid()).bind(id.as_uuid()).bind(role).execute(&pool).await.unwrap();
        }
        for (id, account_id, code) in [
            (store, account, "collection"),
            (other_store, other_account, "collection-other"),
        ] {
            sqlx::query("INSERT INTO merchant.stores (id,merchant_account_id,code,name,status) VALUES ($1,$2,$3,'Collection Store','active')").bind(id.as_uuid()).bind(account_id.as_uuid()).bind(code).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO merchant.sales_channels (id,merchant_account_id,store_id,code,name,kind,is_default,status) VALUES ($1,$2,$3,'web','Web','web',true,'active')")
            .bind(channel.as_uuid()).bind(account.as_uuid()).bind(store.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO merchant.store_currencies (merchant_account_id,store_id,currency) VALUES ($1,$2,'USD')")
            .bind(account.as_uuid()).bind(store.as_uuid()).execute(&pool).await.unwrap();
        for (id, account_id, store_id, handle) in [
            (first, account, store, "first-product"),
            (second, account, store, "second-product"),
            (foreign, other_account, other_store, "foreign-product"),
        ] {
            sqlx::query("INSERT INTO catalog.products (id,merchant_account_id,store_id,handle,title,status) VALUES ($1,$2,$3,$4,$4,'active')").bind(id.as_uuid()).bind(account_id.as_uuid()).bind(store_id.as_uuid()).bind(handle).execute(&pool).await.unwrap();
        }
        for id in [first, second] {
            sqlx::query("INSERT INTO catalog.product_publications (merchant_account_id,store_id,product_id,sales_channel_id) VALUES ($1,$2,$3,$4)").bind(account.as_uuid()).bind(store.as_uuid()).bind(id.as_uuid()).bind(channel.as_uuid()).execute(&pool).await.unwrap();
        }
        for (variant, product, sku) in [
            (first_variant, first, "COL-FIRST"),
            (second_variant, second, "COL-SECOND"),
        ] {
            sqlx::query("INSERT INTO catalog.product_variants (id,merchant_account_id,store_id,product_id,title,sku,status) VALUES ($1,$2,$3,$4,'Default',$5,'active')").bind(variant).bind(account.as_uuid()).bind(store.as_uuid()).bind(product.as_uuid()).bind(sku).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO pricing.price_lists (id,merchant_account_id,store_id,code,name,currency,status) VALUES ($1,$2,$3,'collection-retail','Collection Retail','USD','active')")
            .bind(price_list).bind(account.as_uuid()).bind(store.as_uuid()).execute(&pool).await.unwrap();
        for variant in [first_variant, second_variant] {
            sqlx::query("INSERT INTO pricing.prices (id,merchant_account_id,store_id,price_list_id,product_variant_id,amount_minor) VALUES ($1,$2,$3,$4,$5,1000)").bind(Uuid::now_v7()).bind(account.as_uuid()).bind(store.as_uuid()).bind(price_list).bind(variant).execute(&pool).await.unwrap();
        }

        let base = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/collections",
            account.as_uuid(),
            store.as_uuid()
        );
        let owner_state = test_state(&database_url, owner);
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &base,
                Some(&format!("create-collection-{suffix}")),
                Some(json!({"handle":"featured","title":"Featured"})),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let collection_id = response_json(response).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let detail = format!("{base}/{collection_id}");
        let publication = format!("{detail}/publications/{}", channel.as_uuid());
        let locale_base = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/locales/fr",
            account.as_uuid(),
            store.as_uuid()
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::PUT,
                    &locale_base,
                    Some(&format!("enable-fr-{suffix}")),
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::PUT,
                    &format!("{detail}/translations/fr"),
                    Some(&format!("translate-fr-{suffix}")),
                    Some(json!({"title":"En vedette","description":"Collection française"})),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::PUT,
                    &publication,
                    Some(&format!("premature-{suffix}")),
                    None
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::PUT,
                    &format!("{detail}/products"),
                    Some(&format!("foreign-{suffix}")),
                    Some(json!({"product_ids":[foreign.as_uuid()]}))
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::PUT,
                    &format!("{detail}/products"),
                    Some(&format!("members-{suffix}")),
                    Some(json!({"product_ids":[second.as_uuid(),first.as_uuid()]}))
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &format!("{detail}/activate"),
                    Some(&format!("activate-{suffix}")),
                    None
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::PUT,
                    &publication,
                    Some(&format!("publish-{suffix}")),
                    None
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let body = response_json(
            router(owner_state.clone())
                .oneshot(request(Method::GET, &detail, None, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            body["data"]["products"][0]["product_id"],
            second.as_uuid().to_string()
        );
        assert_eq!(body["data"]["products"][1]["position"], 1);

        let material = publishable_key(&pool, account, store, channel, owner).await;
        let auth = format!("Bearer {}", material.plaintext.expose_secret());
        owner_state
            .search_indexer
            .run_batch(Uuid::now_v7(), 100, owner_state.clock.now())
            .await
            .unwrap();
        let response = router(owner_state.clone())
            .oneshot(
                Request::get("/store/v1/collections/featured")
                    .header("authorization", &auth)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["product_count"], 2);
        let response = router(owner_state.clone())
            .oneshot(
                Request::get("/store/v1/collections/featured?locale=fr")
                    .header("authorization", &auth)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let localized = response_json(response).await;
        assert_eq!(localized["data"]["locale"], "fr");
        assert_eq!(localized["data"]["title"], "En vedette");
        let response = router(owner_state.clone())
            .oneshot(
                Request::get("/store/v1/products?collection=featured")
                    .header("authorization", &auth)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let products = response_json(response).await;
        assert_eq!(products["data"][0]["handle"], "second-product");
        assert_eq!(products["data"][1]["handle"], "first-product");

        let support_state = test_state(&database_url, support);
        assert_eq!(
            router(support_state)
                .oneshot(request(
                    Method::POST,
                    &base,
                    Some(&format!("support-{suffix}")),
                    Some(json!({"handle":"denied","title":"Denied"}))
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );

        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &format!("{detail}/archive"),
                    Some(&format!("archive-{suffix}")),
                    None
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(
                    Request::get("/store/v1/collections/featured")
                        .header("authorization", &auth)
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );

        let id = Uuid::parse_str(&collection_id).unwrap();
        let event_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog.collection_events WHERE collection_id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_count, 5);
        let runtime = owner_state.infrastructure.runtime_pool();
        assert!(sqlx::query("UPDATE catalog.collection_events SET occurred_at=CURRENT_TIMESTAMP WHERE collection_id=$1").bind(id).execute(&runtime).await.is_err());
        assert!(
            sqlx::query("DELETE FROM catalog.collections WHERE id=$1")
                .bind(id)
                .execute(&runtime)
                .await
                .is_err()
        );
    }
}
