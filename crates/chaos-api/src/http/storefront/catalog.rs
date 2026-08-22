use axum::{Router, extract::State, routing::get};
use chaos_application::ports::{
    StorefrontCatalogProduct, StorefrontCatalogVariant, StorefrontMediaAsset,
    StorefrontProductCollection, StorefrontProductOption, StorefrontProductOptionValue,
    StorefrontSelectedOption,
};
use chaos_domain::catalog::ProductId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ApiError, ApiPath, ApiQuery, ApiResponse, ApiState, StorefrontMachine,
    pagination::{CursorKind, decode_cursor, encode_cursor, page_limit, page_meta},
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/products", get(list_products))
        .route("/products/{handle}", get(get_product))
}

#[derive(Deserialize)]
struct CatalogQuery {
    currency: Option<String>,
    locale: Option<String>,
    q: Option<String>,
    collection: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize)]
struct ProductQuery {
    currency: Option<String>,
    locale: Option<String>,
}

#[derive(Deserialize)]
struct ProductPath {
    handle: String,
}

#[derive(Serialize)]
struct StorefrontVariantData {
    id: Uuid,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    requires_shipping: bool,
    price: StorefrontPriceData,
    selected_options: Vec<StorefrontSelectedOptionData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct StorefrontSelectedOptionData {
    option_id: Uuid,
    option_value_id: Uuid,
}

#[derive(Serialize)]
struct StorefrontProductOptionValueData {
    id: Uuid,
    value: String,
    position: u16,
}

#[derive(Serialize)]
struct StorefrontProductOptionData {
    id: Uuid,
    name: String,
    position: u16,
    values: Vec<StorefrontProductOptionValueData>,
}

#[derive(Serialize)]
struct StorefrontPriceData {
    amount_minor: i64,
    currency: String,
    tax_inclusive: bool,
}

#[derive(Serialize)]
struct StorefrontProductData {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    locale: String,
    options: Vec<StorefrontProductOptionData>,
    variants: Vec<StorefrontVariantData>,
    media: Vec<StorefrontMediaData>,
    collections: Vec<StorefrontProductCollectionData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct StorefrontProductCollectionData {
    id: Uuid,
    handle: String,
    title: String,
}

#[derive(Serialize)]
struct StorefrontMediaData {
    id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_variant_id: Option<Uuid>,
    media_type: String,
    kind: &'static str,
    alt_text: String,
    position: u16,
    url: String,
}

async fn list_products(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiQuery(query): ApiQuery<CatalogQuery>,
) -> Result<ApiResponse<Vec<StorefrontProductData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::Product))
        .transpose()?
        .map(ProductId::from_uuid);
    let page = state
        .storefront_catalog
        .list_products(
            &actor,
            query.currency.as_deref(),
            query.locale.as_deref(),
            query.q.as_deref(),
            query.collection.as_deref(),
            after,
            limit,
        )
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::Product))
    });
    Ok(
        ApiResponse::ok(page.items.into_iter().map(product_data).collect())
            .with_meta(page_meta(page.has_more, next_cursor.flatten())),
    )
}

async fn get_product(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiPath(path): ApiPath<ProductPath>,
    ApiQuery(query): ApiQuery<ProductQuery>,
) -> Result<ApiResponse<StorefrontProductData>, ApiError> {
    let product = state
        .storefront_catalog
        .get_product_by_handle(
            &actor,
            query.currency.as_deref(),
            query.locale.as_deref(),
            &path.handle,
        )
        .await?;
    Ok(ApiResponse::ok(product_data(product)))
}

fn product_data(product: StorefrontCatalogProduct) -> StorefrontProductData {
    StorefrontProductData {
        id: product.id.as_uuid(),
        handle: product.handle,
        title: product.title,
        description: product.description,
        locale: product.locale.as_str().into(),
        options: product.options.into_iter().map(option_data).collect(),
        variants: product.variants.into_iter().map(variant_data).collect(),
        media: product.media.into_iter().map(media_data).collect(),
        collections: product
            .collections
            .into_iter()
            .map(collection_ref_data)
            .collect(),
        metadata: product.metadata,
    }
}

fn collection_ref_data(collection: StorefrontProductCollection) -> StorefrontProductCollectionData {
    StorefrontProductCollectionData {
        id: collection.id.as_uuid(),
        handle: collection.handle,
        title: collection.title,
    }
}

fn option_data(option: StorefrontProductOption) -> StorefrontProductOptionData {
    StorefrontProductOptionData {
        id: option.id.as_uuid(),
        name: option.name,
        position: option.position,
        values: option.values.into_iter().map(option_value_data).collect(),
    }
}

fn option_value_data(value: StorefrontProductOptionValue) -> StorefrontProductOptionValueData {
    StorefrontProductOptionValueData {
        id: value.id.as_uuid(),
        value: value.value,
        position: value.position,
    }
}

fn selected_option_data(selection: StorefrontSelectedOption) -> StorefrontSelectedOptionData {
    StorefrontSelectedOptionData {
        option_id: selection.option_id.as_uuid(),
        option_value_id: selection.option_value_id.as_uuid(),
    }
}

fn media_data(media: StorefrontMediaAsset) -> StorefrontMediaData {
    StorefrontMediaData {
        id: media.id.as_uuid(),
        product_variant_id: media.product_variant_id.map(|id| id.as_uuid()),
        media_type: media.media_type,
        kind: media.kind.as_str(),
        alt_text: media.alt_text,
        position: media.position,
        url: media.url,
    }
}

fn variant_data(variant: StorefrontCatalogVariant) -> StorefrontVariantData {
    StorefrontVariantData {
        id: variant.id.as_uuid(),
        title: variant.title,
        sku: variant.sku,
        requires_shipping: variant.requires_shipping,
        price: StorefrontPriceData {
            amount_minor: variant.amount_minor,
            currency: variant.currency.as_str().to_owned(),
            tax_inclusive: variant.tax_inclusive,
        },
        selected_options: variant
            .selected_options
            .into_iter()
            .map(selected_option_data)
            .collect(),
        metadata: variant.metadata,
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use chaos_application::ports::{
        GeneratedPublishableKeyMaterial, PublishableKeyMaterialGenerator,
    };
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        identity::UserId,
        store::{PublishableKeyId, SalesChannelId, StoreId},
    };
    use chaos_infrastructure::repositories::SecurePublishableKeyMaterialGenerator;
    use secrecy::ExposeSecret;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use tower::ServiceExt;

    use crate::http::{
        router,
        test_support::{response_json, test_state},
    };

    use super::*;

    async fn insert_publishable_key(
        pool: &PgPool,
        store_id: StoreId,
        user_id: UserId,
    ) -> GeneratedPublishableKeyMaterial {
        let material = SecurePublishableKeyMaterialGenerator.generate();
        let key_id = PublishableKeyId::new();
        sqlx::query(
            "INSERT INTO commerce.publishable_keys \
             (id, store_id, key_identifier, secret_digest, \
              display_suffix, name, created_by_user_id) \
             VALUES ($1, $2, $3, $4, $5, 'Storefront HTTP', $6)",
        )
        .bind(key_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(&material.key_identifier)
        .bind(material.secret_digest.as_slice())
        .bind(&material.display_suffix)
        .bind(user_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
        material
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn storefront_http_serves_only_public_contract_fields_through_publishable_authentication()
    {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let user_id = UserId::new();
        let store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let price_list_id = Uuid::now_v7();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id.as_uuid())
            .bind(format!("storefront-http-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO commerce.stores \
             (id, code, name, status) \
             VALUES ($1, $2, 'Storefront HTTP', 'active')",
        )
        .bind(store_id.as_uuid())
        .bind(format!("storefront-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.sales_channels \
             (id, store_id, code, name, is_default) \
             VALUES ($1, $2, 'web', 'Web', true)",
        )
        .bind(channel_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.store_currencies (store_id, currency) \
             VALUES ($1, 'USD')",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.products \
             (id, store_id, handle, title, description, status) \
             VALUES ($1, $2, 'public-shirt', 'Public Shirt', 'Public description', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.product_variants \
             (id, store_id, product_id, title, sku, status) \
             VALUES ($1, $2, $3, 'Default', 'PUBLIC-SHIRT', 'active')",
        )
        .bind(variant_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.product_publications \
             (store_id, product_id, sales_channel_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(channel_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.price_lists \
             (id, store_id, code, name, currency, status) \
             VALUES ($1, $2, 'public-retail', 'Public Retail', 'USD', 'active')",
        )
        .bind(price_list_id)
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.prices \
             (id, store_id, price_list_id, product_variant_id, amount_minor) \
             VALUES ($1, $2, $3, $4, 4200)",
        )
        .bind(Uuid::now_v7())
        .bind(store_id.as_uuid())
        .bind(price_list_id)
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        for locale in ["zh", "zh-CN", "fr"] {
            sqlx::query(
                "INSERT INTO commerce.store_locales \
                 (store_id, locale, created_by_user_id, created_at) \
                 VALUES ($1, $2, $3, CURRENT_TIMESTAMP)",
            )
            .bind(store_id.as_uuid())
            .bind(locale)
            .bind(user_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (locale, title) in [("zh", "Language Shirt"), ("zh-CN", "Regional Shirt")] {
            sqlx::query(
                "INSERT INTO commerce.product_translations \
                 (store_id, product_id, locale, title, description, \
                  updated_by_user_id, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, 'Localized description', $5, \
                         CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            )
            .bind(store_id.as_uuid())
            .bind(product_id.as_uuid())
            .bind(locale)
            .bind(title)
            .bind(user_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO commerce.product_variant_translations \
             (store_id, product_id, product_variant_id, locale, title, \
              updated_by_user_id, created_at, updated_at) \
             VALUES ($1, $2, $3, 'zh', 'Localized Default', $4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(variant_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        let material = insert_publishable_key(&owner_pool, store_id, user_id).await;
        let state = test_state(&database_url, user_id);
        assert!(
            chaos_infrastructure::repositories::PostgresSearchIndexer::new(
                state.infrastructure.runtime_pool(),
            )
            .run_batch(100, state.clock.now())
            .await
            .unwrap()
                >= 2
        );
        let authorize = format!("Bearer {}", material.plaintext.expose_secret());

        let response = router(state.clone())
            .oneshot(
                Request::get("/store/v1/products")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"][0]["handle"], "public-shirt");
        assert_eq!(body["data"][0]["locale"], "en-US");
        assert_eq!(
            body["data"][0]["variants"][0]["price"]["amount_minor"],
            4200
        );
        assert!(body["data"][0].get("status").is_none());
        assert!(body["data"][0].get("merchant_account_id").is_none());

        let response = router(state.clone())
            .oneshot(
                Request::get("/store/v1/products/public-shirt")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router(state.clone())
            .oneshot(
                Request::get("/store/v1/products/public-shirt?locale=zh-CN")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let localized = response_json(response).await;
        assert_eq!(localized["data"]["locale"], "zh-CN");
        assert_eq!(localized["data"]["title"], "Regional Shirt");
        assert_eq!(
            localized["data"]["variants"][0]["title"],
            "Localized Default"
        );

        let response = router(state.clone())
            .oneshot(
                Request::get("/store/v1/products/public-shirt?locale=fr")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let fallback = response_json(response).await;
        assert_eq!(fallback["data"]["locale"], "fr");
        assert_eq!(fallback["data"]["title"], "Public Shirt");

        assert_eq!(
            router(state.clone())
                .oneshot(
                    Request::get("/store/v1/products/public-shirt?locale=es")
                        .header("authorization", &authorize)
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let response = router(state.clone())
            .oneshot(
                Request::get("/store/v1/products?currency=usd")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(state)
            .oneshot(
                Request::get("/store/v1/products/missing-product")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
