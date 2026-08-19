use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::post,
};
use chaos_application::{
    ApplicationError,
    catalog::{CreateMediaAssetInput, MediaAssetActionInput, RefreshMediaUploadInput},
    ports::{AdminActor, IdempotencyRequest, MediaAssetItem, MediaUploadRequest},
};
use chaos_domain::{
    catalog::{MediaAssetId, ProductId, ProductVariantId},
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
        .route(
            "/stores/{store_id}/products/{product_id}/media",
            post(create_media).get(list_media),
        )
        .route(
            "/stores/{store_id}/products/{product_id}/media/{media_asset_id}/upload",
            post(refresh_upload),
        )
        .route(
            "/stores/{store_id}/products/{product_id}/media/{media_asset_id}/complete",
            post(complete_media),
        )
        .route(
            "/stores/{store_id}/products/{product_id}/media/{media_asset_id}/archive",
            post(archive_media),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}

#[derive(Deserialize)]
struct ProductPath {
    store_id: Uuid,
    product_id: Uuid,
}
#[derive(Deserialize)]
struct MediaPath {
    store_id: Uuid,
    product_id: Uuid,
    media_asset_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateMediaBody {
    product_variant_id: Option<Uuid>,
    file_name: String,
    media_type: String,
    byte_size: u64,
    sha256: String,
    #[serde(default)]
    alt_text: String,
    position: u16,
}

#[derive(Serialize)]
struct MediaData {
    id: Uuid,
    product_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_variant_id: Option<Uuid>,
    file_name: String,
    media_type: String,
    kind: &'static str,
    byte_size: u64,
    sha256: String,
    alt_text: String,
    position: u16,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_url: Option<String>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}
#[derive(Serialize)]
struct UploadData {
    method: &'static str,
    url: String,
    headers: Vec<UploadHeaderData>,
    expires_at: ApiDateTime,
}
#[derive(Serialize)]
struct UploadHeaderData {
    name: String,
    value: String,
}
#[derive(Serialize)]
struct CreatedMediaData {
    asset: MediaData,
    upload: UploadData,
}

async fn create_media(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductPath>,
    ApiJson(body): ApiJson<CreateMediaBody>,
) -> Result<Response, ApiError> {
    let request = mutation(&headers, &(path.store_id, path.product_id, &body))?;
    let created = state
        .media_administration
        .create(CreateMediaAssetInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            product_id: ProductId::from_uuid(path.product_id),
            product_variant_id: body.product_variant_id.map(ProductVariantId::from_uuid),
            file_name: body.file_name,
            media_type: body.media_type,
            byte_size: body.byte_size,
            sha256_hex: body.sha256,
            alt_text: body.alt_text,
            position: body.position,
            idempotency: request,
            now: state.clock.now(),
        })
        .await?;
    no_store(ApiResponse::created(CreatedMediaData {
        asset: media_data(created.asset),
        upload: upload_data(created.upload),
    }))
}

async fn list_media(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<ProductPath>,
) -> Result<ApiResponse<Vec<MediaData>>, ApiError> {
    let items = state
        .media_administration
        .list(
            AdminActor::Store(actor),
            StoreId::from_uuid(path.store_id),
            ProductId::from_uuid(path.product_id),
        )
        .await?;
    Ok(ApiResponse::ok(items.into_iter().map(media_data).collect()))
}

async fn refresh_upload(
    State(state): State<ApiState>,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<MediaPath>,
) -> Result<Response, ApiError> {
    let upload = state
        .media_administration
        .refresh_upload(RefreshMediaUploadInput {
            actor: AdminActor::Store(actor),
            store_id: StoreId::from_uuid(path.store_id),
            product_id: ProductId::from_uuid(path.product_id),
            media_asset_id: MediaAssetId::from_uuid(path.media_asset_id),
            now: state.clock.now(),
        })
        .await?;
    no_store(ApiResponse::ok(upload_data(upload)))
}

async fn complete_media(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<MediaPath>,
) -> Result<ApiResponse<MediaData>, ApiError> {
    action(state, headers, actor, path, true).await
}
async fn archive_media(
    State(state): State<ApiState>,
    headers: HeaderMap,
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<MediaPath>,
) -> Result<ApiResponse<MediaData>, ApiError> {
    action(state, headers, actor, path, false).await
}
async fn action(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::StoreActor,
    path: MediaPath,
    complete: bool,
) -> Result<ApiResponse<MediaData>, ApiError> {
    let input = MediaAssetActionInput {
        actor: AdminActor::Store(actor),
        store_id: StoreId::from_uuid(path.store_id),
        product_id: ProductId::from_uuid(path.product_id),
        media_asset_id: MediaAssetId::from_uuid(path.media_asset_id),
        idempotency: mutation(
            &headers,
            &(
                path.store_id,
                path.product_id,
                path.media_asset_id,
                complete,
            ),
        )?,
        now: state.clock.now(),
    };
    let item = if complete {
        state.media_administration.complete(input).await?
    } else {
        state.media_administration.archive(input).await?
    };
    Ok(ApiResponse::ok(media_data(item)))
}

fn media_data(value: MediaAssetItem) -> MediaData {
    MediaData {
        id: value.id.as_uuid(),
        product_id: value.product_id.as_uuid(),
        product_variant_id: value.product_variant_id.map(ProductVariantId::as_uuid),
        file_name: value.file_name,
        media_type: value.media_type,
        kind: value.kind.as_str(),
        byte_size: value.byte_size,
        sha256: value.sha256_hex,
        alt_text: value.alt_text,
        position: value.position,
        status: value.status.as_str(),
        public_url: value.public_url,
        created_at: value.created_at.into(),
        updated_at: value.updated_at.into(),
    }
}
fn upload_data(value: MediaUploadRequest) -> UploadData {
    UploadData {
        method: value.method,
        url: value.url,
        headers: value
            .headers
            .into_iter()
            .map(|(name, value)| UploadHeaderData { name, value })
            .collect(),
        expires_at: value.expires_at.into(),
    }
}
fn no_store<T: Serialize>(response: ApiResponse<T>) -> Result<Response, ApiError> {
    let mut response = response.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
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
    use std::sync::{Arc, RwLock};

    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use chaos_application::{
        ApplicationError,
        catalog::MediaAdministration,
        ports::{
            ApiKeyMaterialGenerator, GeneratedApiKeyMaterial, MediaStorage, MediaUploadRequest,
            StoredMediaObject,
        },
    };
    use chaos_domain::{
        catalog::MediaDescriptor,
        identity::UserId,
        merchant::{ApiKeyClass, ApiKeyId, SalesChannelId},
    };
    use chaos_infrastructure::repositories::{
        PostgresMediaAssetRepository, SecureApiKeyMaterialGenerator,
    };
    use secrecy::ExposeSecret;
    use serde_json::json;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use time::OffsetDateTime;
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{request, response_json, test_state},
        router,
    };

    use super::*;

    struct FakeStorage(RwLock<Option<StoredMediaObject>>);

    #[async_trait]
    impl MediaStorage for FakeStorage {
        async fn prepare_upload(
            &self,
            _: &str,
            descriptor: &MediaDescriptor,
            _: std::time::Duration,
            expires_at: OffsetDateTime,
        ) -> Result<MediaUploadRequest, ApplicationError> {
            Ok(MediaUploadRequest {
                method: "PUT",
                url: "https://uploads.example.test/signed".into(),
                headers: vec![
                    ("content-type".into(), descriptor.media_type().into()),
                    ("x-amz-checksum-sha256".into(), "signed-checksum".into()),
                ],
                expires_at,
            })
        }

        async fn inspect(&self, _: &str) -> Result<Option<StoredMediaObject>, ApplicationError> {
            Ok(self.0.read().unwrap().clone())
        }

        fn public_url(&self, object_key: &str) -> Result<String, ApplicationError> {
            Ok(format!("https://media.example.test/{object_key}"))
        }
    }

    async fn publishable_key(
        pool: &PgPool,
        store_id: StoreId,
        channel_id: SalesChannelId,
        user_id: UserId,
    ) -> GeneratedApiKeyMaterial {
        let material = SecureApiKeyMaterialGenerator.generate(ApiKeyClass::Publishable);
        let key_id = ApiKeyId::new();
        sqlx::query("INSERT INTO merchant.api_keys (id,store_id,sales_channel_id,key_identifier,secret_digest,display_suffix,name,class,created_by_user_id) VALUES ($1,$2,$3,$4,$5,$6,'Media Storefront','publishable',$7)")
            .bind(key_id.as_uuid()).bind(store_id.as_uuid()).bind(channel_id.as_uuid()).bind(&material.key_identifier).bind(material.secret_digest.as_slice()).bind(&material.display_suffix).bind(user_id.as_uuid()).execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO merchant.api_key_scopes (api_key_id,scope) VALUES ($1,'catalog:read')",
        )
        .bind(key_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
        material
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn media_http_verifies_upload_before_storefront_visibility() {
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
        let other_store = StoreId::new();
        let channel = SalesChannelId::new();
        let product = ProductId::new();
        let variant = ProductVariantId::new();
        let foreign_variant = ProductVariantId::new();
        let price_list = Uuid::now_v7();
        let suffix = Uuid::now_v7().simple().to_string();
        let digest = "ab".repeat(32);

        for (id, role) in [(owner, "owner"), (support, "support")] {
            sqlx::query("INSERT INTO identity.users (id,email) VALUES ($1,$2)")
                .bind(id.as_uuid())
                .bind(format!("media-{role}-{suffix}@example.com"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO merchant.stores (id,code,name,status) VALUES ($1,$2,'Media Store','active')")
            .bind(store.as_uuid()).bind(format!("media-{}", &suffix[12..28])).execute(&pool).await.unwrap();
        for (id, role) in [(owner, "owner"), (support, "member")] {
            sqlx::query("INSERT INTO merchant.store_memberships (store_id,user_id,role) VALUES ($1,$2,$3::merchant.store_role)")
                .bind(store.as_uuid()).bind(id.as_uuid()).bind(role).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO merchant.sales_channels (id,store_id,code,name,kind,is_default,status) VALUES ($1,$2,'web','Web','web',true,'active')")
            .bind(channel.as_uuid()).bind(store.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO merchant.store_currencies (store_id,currency) VALUES ($1,'USD')")
            .bind(store.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO catalog.products (id,store_id,handle,title,status) VALUES ($1,$2,'media-product','Media Product','active')")
            .bind(product.as_uuid()).bind(store.as_uuid()).execute(&pool).await.unwrap();
        for (id, title) in [
            (variant, "Default"),
            (foreign_variant, "Foreign Product Variant"),
        ] {
            let parent = if id == variant {
                product
            } else {
                ProductId::new()
            };
            if id == foreign_variant {
                sqlx::query("INSERT INTO catalog.products (id,store_id,handle,title,status) VALUES ($1,$2,'foreign-media-product','Foreign Media Product','active')")
                    .bind(parent.as_uuid()).bind(store.as_uuid()).execute(&pool).await.unwrap();
            }
            sqlx::query("INSERT INTO catalog.product_variants (id,store_id,product_id,title,status) VALUES ($1,$2,$3,$4,'active')")
                .bind(id.as_uuid()).bind(store.as_uuid()).bind(parent.as_uuid()).bind(title).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO catalog.product_publications (store_id,product_id,sales_channel_id) VALUES ($1,$2,$3)")
            .bind(store.as_uuid()).bind(product.as_uuid()).bind(channel.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO pricing.price_lists (id,store_id,code,name,currency,status) VALUES ($1,$2,'media-retail','Media Retail','USD','active')")
            .bind(price_list).bind(store.as_uuid()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO pricing.prices (id,store_id,price_list_id,product_variant_id,amount_minor) VALUES ($1,$2,$3,$4,1000)")
            .bind(Uuid::now_v7()).bind(store.as_uuid()).bind(price_list).bind(variant.as_uuid()).execute(&pool).await.unwrap();

        let storage = Arc::new(FakeStorage(RwLock::new(None)));
        let mut owner_state = test_state(&database_url, owner);
        owner_state.media_administration = Arc::new(MediaAdministration::new(
            Arc::new(PostgresMediaAssetRepository::new(
                owner_state.infrastructure.runtime_pool(),
            )),
            storage.clone(),
        ));
        let base = format!(
            "/admin/v1/stores/{}/products/{}/media",
            store.as_uuid(),
            product.as_uuid()
        );
        let body = json!({"product_variant_id":variant.as_uuid(),"file_name":"hero.webp","media_type":"image/webp","byte_size":1024,"sha256":digest,"alt_text":"Product hero","position":0});
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &base,
                Some(&format!("create-{suffix}")),
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let created = response_json(response).await;
        assert_eq!(created["data"]["asset"]["status"], "pending_upload");
        assert!(created["data"]["asset"].get("public_url").is_none());
        assert_eq!(created["data"]["upload"]["method"], "PUT");
        let media_id = created["data"]["asset"]["id"].as_str().unwrap().to_owned();

        let mut invalid = body;
        invalid["product_variant_id"] = json!(foreign_variant.as_uuid());
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &base,
                    Some(&format!("foreign-{suffix}")),
                    Some(invalid)
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let complete = format!("{base}/{media_id}/complete");
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &complete,
                    Some(&format!("missing-{suffix}")),
                    None
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        *storage.0.write().unwrap() = Some(StoredMediaObject {
            media_type: "image/webp".into(),
            byte_size: 1025,
            sha256_hex: digest.clone(),
        });
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &complete,
                    Some(&format!("mismatch-{suffix}")),
                    None
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        *storage.0.write().unwrap() = Some(StoredMediaObject {
            media_type: "image/webp".into(),
            byte_size: 1024,
            sha256_hex: digest,
        });
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &complete,
                Some(&format!("ready-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["status"], "ready");

        let locale = format!("/admin/v1/stores/{}/locales/de", store.as_uuid());
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::PUT,
                    &locale,
                    Some(&format!("enable-de-{suffix}")),
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
                    &format!("{base}/{media_id}/translations/de"),
                    Some(&format!("translate-de-{suffix}")),
                    Some(json!({"alt_text":"Produktbild"})),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let material = publishable_key(&pool, store, channel, owner).await;
        let auth = format!("Bearer {}", material.plaintext.expose_secret());
        let storefront = || {
            Request::get("/store/v1/products/media-product")
                .header("authorization", &auth)
                .body(Body::empty())
                .unwrap()
        };
        let response = router(owner_state.clone())
            .oneshot(storefront())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["media"][0]["alt_text"],
            "Product hero"
        );
        let response = router(owner_state.clone())
            .oneshot(
                Request::get("/store/v1/products/media-product?locale=de")
                    .header("authorization", &auth)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["media"][0]["alt_text"],
            "Produktbild"
        );

        let support_state = test_state(&database_url, support);
        let archive_response = router(support_state)
            .oneshot(request(
                Method::POST,
                &format!("{base}/{media_id}/archive"),
                Some(&format!("support-{suffix}")),
                None,
            ))
            .await
            .unwrap()
            .status();
        assert_eq!(archive_response, StatusCode::OK);
        assert_eq!(
            router(owner_state.clone())
                .oneshot(request(
                    Method::POST,
                    &format!("{base}/{media_id}/archive"),
                    Some(&format!("archive-{suffix}")),
                    None
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert!(
            response_json(
                router(owner_state.clone())
                    .oneshot(storefront())
                    .await
                    .unwrap()
            )
            .await["data"]["media"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let event_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM catalog.media_events WHERE media_asset_id=$1")
                .bind(Uuid::parse_str(&media_id).unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(event_count, 3);
        let runtime = owner_state.infrastructure.runtime_pool();
        assert!(sqlx::query("UPDATE catalog.media_events SET occurred_at=CURRENT_TIMESTAMP WHERE media_asset_id=$1").bind(Uuid::parse_str(&media_id).unwrap()).execute(&runtime).await.is_err());
        assert!(
            sqlx::query("DELETE FROM catalog.media_assets WHERE id=$1")
                .bind(Uuid::parse_str(&media_id).unwrap())
                .execute(&runtime)
                .await
                .is_err()
        );

        let mut transaction = runtime.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.store_id',$1,true)")
            .bind(other_store.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .unwrap();
        let visible: i64 =
            sqlx::query_scalar("SELECT count(*) FROM catalog.media_assets WHERE id=$1")
                .bind(Uuid::parse_str(&media_id).unwrap())
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        assert_eq!(visible, 0);
        transaction.rollback().await.unwrap();
    }
}
