use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post, put},
};
use chaos_application::{
    ApplicationError,
    catalog::{
        MediaTranslationActionInput, ProductVariantTranslationInput, StoreLocaleInput,
        TranslationActionInput, UpsertCollectionTranslationInput, UpsertMediaTranslationInput,
        UpsertProductTranslationInput,
    },
    ports::{
        AdminActor, CollectionTranslation, IdempotencyRequest, MediaTranslation,
        ProductTranslation, StoreLocaleConfiguration,
    },
};
use chaos_domain::{
    catalog::{CollectionId, MediaAssetId, ProductId, ProductVariantId},
    merchant::StoreId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiResponse, ApiState, StoreContext,
    merchant::idempotency_key,
};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/stores/{store_id}/locales", get(store_locales))
        .route(
            "/stores/{store_id}/locales/{locale}",
            put(enable_locale).delete(disable_locale),
        )
        .route(
            "/stores/{store_id}/locales/{locale}/default",
            post(set_default_locale),
        )
        .route(
            "/stores/{store_id}/products/{product_id}/translations/{locale}",
            get(get_product_translation)
                .put(upsert_product_translation)
                .delete(remove_product_translation),
        )
        .route(
            "/stores/{store_id}/collections/{collection_id}/translations/{locale}",
            get(get_collection_translation)
                .put(upsert_collection_translation)
                .delete(remove_collection_translation),
        )
        .route(
            "/stores/{store_id}/products/{product_id}/media/{media_asset_id}/translations/{locale}",
            get(get_media_translation)
                .put(upsert_media_translation)
                .delete(remove_media_translation),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    store_id: Uuid,
}
#[derive(Deserialize)]
struct LocalePath {
    store_id: Uuid,
    locale: String,
}
#[derive(Deserialize)]
struct ProductTranslationPath {
    store_id: Uuid,
    product_id: Uuid,
    locale: String,
}
#[derive(Deserialize)]
struct CollectionTranslationPath {
    store_id: Uuid,
    collection_id: Uuid,
    locale: String,
}
#[derive(Deserialize)]
struct MediaTranslationPath {
    store_id: Uuid,
    product_id: Uuid,
    media_asset_id: Uuid,
    locale: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProductTranslationBody {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    variants: Vec<ProductVariantTranslationBody>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProductVariantTranslationBody {
    product_variant_id: Uuid,
    title: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentTranslationBody {
    title: String,
    #[serde(default)]
    description: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MediaTranslationBody {
    #[serde(default)]
    alt_text: String,
}

#[derive(Serialize)]
struct StoreLocalesData {
    default_locale: String,
    enabled_locales: Vec<String>,
}
#[derive(Serialize)]
struct ProductTranslationData {
    locale: String,
    title: String,
    description: String,
    variants: Vec<ProductVariantTranslationData>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}
#[derive(Serialize)]
struct ProductVariantTranslationData {
    product_variant_id: Uuid,
    title: String,
}
#[derive(Serialize)]
struct ContentTranslationData {
    locale: String,
    title: String,
    description: String,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}
#[derive(Serialize)]
struct MediaTranslationData {
    locale: String,
    alt_text: String,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}
#[derive(Serialize)]
struct RemovedData {
    removed: bool,
}

async fn store_locales(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<StoreLocalesData>, ApiError> {
    Ok(ApiResponse::ok(locale_data(
        state
            .catalog_localization
            .store_locales(AdminActor::Store(actor), StoreId::from_uuid(path.store_id))
            .await?,
    )))
}

async fn enable_locale(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<LocalePath>,
) -> Result<ApiResponse<StoreLocalesData>, ApiError> {
    locale_action(state, headers, actor, path, "enable").await
}
async fn set_default_locale(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<LocalePath>,
) -> Result<ApiResponse<StoreLocalesData>, ApiError> {
    locale_action(state, headers, actor, path, "default").await
}
async fn disable_locale(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<LocalePath>,
) -> Result<ApiResponse<StoreLocalesData>, ApiError> {
    locale_action(state, headers, actor, path, "disable").await
}
async fn locale_action(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::StoreActor,
    path: LocalePath,
    action: &'static str,
) -> Result<ApiResponse<StoreLocalesData>, ApiError> {
    let input = StoreLocaleInput {
        actor: AdminActor::Store(actor),
        store_id: StoreId::from_uuid(path.store_id),
        locale: path.locale,
        idempotency: mutation(&headers, &(path.store_id, action))?,
        now: state.clock.now(),
    };
    let value = match action {
        "enable" => state.catalog_localization.enable_locale(input).await?,
        "default" => state.catalog_localization.set_default_locale(input).await?,
        "disable" => state.catalog_localization.disable_locale(input).await?,
        _ => unreachable!(),
    };
    Ok(ApiResponse::ok(locale_data(value)))
}

async fn get_product_translation(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductTranslationPath>,
) -> Result<ApiResponse<ProductTranslationData>, ApiError> {
    Ok(ApiResponse::ok(product_translation_data(
        state
            .catalog_localization
            .product_translation(
                AdminActor::Store(actor),
                StoreId::from_uuid(path.store_id),
                ProductId::from_uuid(path.product_id),
                &path.locale,
            )
            .await?,
    )))
}

async fn upsert_product_translation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductTranslationPath>,
    ApiJson(body): ApiJson<ProductTranslationBody>,
) -> Result<ApiResponse<ProductTranslationData>, ApiError> {
    let idempotency = mutation(
        &headers,
        &(path.store_id, path.product_id, &path.locale, &body),
    )?;
    let translation = state
        .catalog_localization
        .upsert_product_translation(UpsertProductTranslationInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            product_id: ProductId::from_uuid(path.product_id),
            locale: path.locale,
            title: body.title,
            description: body.description,
            variants: body
                .variants
                .into_iter()
                .map(|value| ProductVariantTranslationInput {
                    product_variant_id: ProductVariantId::from_uuid(value.product_variant_id),
                    title: value.title,
                })
                .collect(),
            idempotency,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(product_translation_data(translation)))
}

async fn remove_product_translation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductTranslationPath>,
) -> Result<ApiResponse<RemovedData>, ApiError> {
    let idempotency = mutation(
        &headers,
        &(path.store_id, path.product_id, &path.locale, "remove"),
    )?;
    state
        .catalog_localization
        .remove_product_translation(TranslationActionInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            resource_id: ProductId::from_uuid(path.product_id),
            locale: path.locale,
            idempotency,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(RemovedData { removed: true }))
}

async fn get_collection_translation(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<CollectionTranslationPath>,
) -> Result<ApiResponse<ContentTranslationData>, ApiError> {
    Ok(ApiResponse::ok(collection_translation_data(
        state
            .catalog_localization
            .collection_translation(
                AdminActor::Store(actor),
                StoreId::from_uuid(path.store_id),
                CollectionId::from_uuid(path.collection_id),
                &path.locale,
            )
            .await?,
    )))
}
async fn upsert_collection_translation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<CollectionTranslationPath>,
    ApiJson(body): ApiJson<ContentTranslationBody>,
) -> Result<ApiResponse<ContentTranslationData>, ApiError> {
    let idempotency = mutation(
        &headers,
        &(path.store_id, path.collection_id, &path.locale, &body),
    )?;
    let value = state
        .catalog_localization
        .upsert_collection_translation(UpsertCollectionTranslationInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            collection_id: CollectionId::from_uuid(path.collection_id),
            locale: path.locale,
            title: body.title,
            description: body.description,
            idempotency,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(collection_translation_data(value)))
}
async fn remove_collection_translation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<CollectionTranslationPath>,
) -> Result<ApiResponse<RemovedData>, ApiError> {
    let idempotency = mutation(
        &headers,
        &(path.store_id, path.collection_id, &path.locale, "remove"),
    )?;
    state
        .catalog_localization
        .remove_collection_translation(TranslationActionInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            resource_id: CollectionId::from_uuid(path.collection_id),
            locale: path.locale,
            idempotency,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(RemovedData { removed: true }))
}

async fn get_media_translation(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<MediaTranslationPath>,
) -> Result<ApiResponse<MediaTranslationData>, ApiError> {
    Ok(ApiResponse::ok(media_translation_data(
        state
            .catalog_localization
            .media_translation(
                AdminActor::Store(actor),
                StoreId::from_uuid(path.store_id),
                ProductId::from_uuid(path.product_id),
                MediaAssetId::from_uuid(path.media_asset_id),
                &path.locale,
            )
            .await?,
    )))
}
async fn upsert_media_translation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<MediaTranslationPath>,
    ApiJson(body): ApiJson<MediaTranslationBody>,
) -> Result<ApiResponse<MediaTranslationData>, ApiError> {
    let idempotency = mutation(
        &headers,
        &(
            path.store_id,
            path.product_id,
            path.media_asset_id,
            &path.locale,
            &body,
        ),
    )?;
    let value = state
        .catalog_localization
        .upsert_media_translation(UpsertMediaTranslationInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            product_id: ProductId::from_uuid(path.product_id),
            media_asset_id: MediaAssetId::from_uuid(path.media_asset_id),
            locale: path.locale,
            alt_text: body.alt_text,
            idempotency,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(media_translation_data(value)))
}
async fn remove_media_translation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<MediaTranslationPath>,
) -> Result<ApiResponse<RemovedData>, ApiError> {
    let idempotency = mutation(
        &headers,
        &(
            path.store_id,
            path.product_id,
            path.media_asset_id,
            &path.locale,
            "remove",
        ),
    )?;
    state
        .catalog_localization
        .remove_media_translation(MediaTranslationActionInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            product_id: ProductId::from_uuid(path.product_id),
            media_asset_id: MediaAssetId::from_uuid(path.media_asset_id),
            locale: path.locale,
            idempotency,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::ok(RemovedData { removed: true }))
}

fn locale_data(value: StoreLocaleConfiguration) -> StoreLocalesData {
    StoreLocalesData {
        default_locale: value.default_locale.as_str().into(),
        enabled_locales: value
            .enabled_locales
            .into_iter()
            .map(|locale| locale.as_str().into())
            .collect(),
    }
}
fn product_translation_data(value: ProductTranslation) -> ProductTranslationData {
    ProductTranslationData {
        locale: value.content.locale().as_str().into(),
        title: value.content.title().into(),
        description: value.content.description().into(),
        variants: value
            .variants
            .into_iter()
            .map(|variant| ProductVariantTranslationData {
                product_variant_id: variant.product_variant_id.as_uuid(),
                title: variant.title.as_str().into(),
            })
            .collect(),
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    }
}
fn collection_translation_data(value: CollectionTranslation) -> ContentTranslationData {
    ContentTranslationData {
        locale: value.content.locale().as_str().into(),
        title: value.content.title().into(),
        description: value.content.description().into(),
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    }
}
fn media_translation_data(value: MediaTranslation) -> MediaTranslationData {
    MediaTranslationData {
        locale: value.locale.as_str().into(),
        alt_text: value.alt_text.as_str().into(),
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    }
}
fn mutation<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(value)
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode};
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        identity::UserId,
        merchant::StoreId,
    };
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn localization_http_enforces_locale_translation_audit_and_authorization() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let owner = UserId::new();
        let support = UserId::new();
        let store = StoreId::new();
        let product = ProductId::new();
        let variant = ProductVariantId::new();
        let suffix = uuid::Uuid::now_v7().simple().to_string();

        for (user, label) in [(owner, "owner"), (support, "support")] {
            sqlx::query("INSERT INTO identity.users (id,email) VALUES ($1,$2)")
                .bind(user.as_uuid())
                .bind(format!("localization-{label}-{suffix}@example.com"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.stores (id,code,name,status) \
             VALUES ($1,$2,'Localization','active')",
        )
        .bind(store.as_uuid())
        .bind(format!("localization-{}", &suffix[12..28]))
        .execute(&pool)
        .await
        .unwrap();
        for (user, role) in [(owner, "owner"), (support, "member")] {
            sqlx::query(
                "INSERT INTO merchant.store_memberships \
                 (store_id,user_id,role) \
                 VALUES ($1,$2,$3::merchant.store_role)",
            )
            .bind(store.as_uuid())
            .bind(user.as_uuid())
            .bind(role)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO catalog.products \
             (id,store_id,handle,title,status) \
             VALUES ($1,$2,'localized-product','Canonical Product','active')",
        )
        .bind(product.as_uuid())
        .bind(store.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id,store_id,product_id,title,status) \
             VALUES ($1,$2,$3,'Canonical Variant','active')",
        )
        .bind(variant.as_uuid())
        .bind(store.as_uuid())
        .bind(product.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

        let locales = format!("/admin/v1/stores/{}/locales", store.as_uuid());
        let owner_state = test_state(&database_url, owner);
        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &locales, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["default_locale"],
            "en-US"
        );

        let zh = format!("{locales}/zh");
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &zh,
                Some(&format!("enable-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["enabled_locales"][1],
            "zh"
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::PUT,
                    &format!("{locales}/not_a_locale"),
                    Some(&format!("invalid-{suffix}")),
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::DELETE,
                    &format!("{locales}/en-US"),
                    Some(&format!("default-{suffix}")),
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );

        let translation = format!(
            "/admin/v1/stores/{}/products/{}/translations/zh",
            store.as_uuid(),
            product.as_uuid()
        );
        let body = json!({
            "title":"Localized Product",
            "description":"Localized description",
            "variants":[{"product_variant_id":variant.as_uuid(),"title":"Localized Variant"}]
        });
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &translation,
                Some(&format!("translate-{suffix}")),
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["title"],
            "Localized Product"
        );
        let replay = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &translation,
                Some(&format!("translate-{suffix}")),
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(
            router(test_state(&database_url, support))
                .oneshot(request(
                    Method::DELETE,
                    &translation,
                    Some(&format!("support-{suffix}")),
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
                    &translation.replace("/zh", "/en-US"),
                    Some(&format!("canonical-{suffix}")),
                    Some(json!({"title":"Forbidden canonical","variants":[]})),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::DELETE,
                    &translation,
                    Some(&format!("remove-{suffix}")),
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(Method::GET, &translation, None, None))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::GET,
                    &locales.replace(
                        &store.as_uuid().to_string(),
                        &uuid::Uuid::now_v7().to_string()
                    ),
                    None,
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );

        let locale_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM merchant.store_locale_events WHERE store_id=$1",
        )
        .bind(store.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        let translation_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog.product_translation_events WHERE product_id=$1",
        )
        .bind(product.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(locale_events, 1);
        assert_eq!(translation_events, 2);
        let runtime_pool = owner_state.infrastructure.runtime_pool();
        assert!(
            sqlx::query(
                "UPDATE catalog.product_translation_events \
                 SET occurred_at=CURRENT_TIMESTAMP WHERE product_id=$1",
            )
            .bind(product.as_uuid())
            .execute(&runtime_pool)
            .await
            .is_err()
        );
    }
}
