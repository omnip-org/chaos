use axum::{Router, extract::State, routing::get};
use chaos_application::ports::{StorefrontCatalogProduct, StorefrontCatalogVariant};
use chaos_domain::catalog::ProductId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ApiError, ApiPath, ApiQuery, ApiResponse, ApiState, StorefrontMachine,
    merchant::{CursorKind, decode_cursor, encode_cursor, page_limit, page_meta},
};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/products", get(list_products))
        .route("/products/{handle}", get(get_product))
}

#[derive(Deserialize)]
struct CatalogQuery {
    currency: Option<String>,
    q: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize)]
struct ProductQuery {
    currency: Option<String>,
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
    variants: Vec<StorefrontVariantData>,
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
        .map(|cursor| decode_cursor(cursor, CursorKind::StorefrontProduct))
        .transpose()?
        .map(ProductId::from_uuid);
    let page = state
        .storefront_catalog
        .list_products(
            &actor,
            query.currency.as_deref(),
            query.q.as_deref(),
            after,
            limit,
        )
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::StorefrontProduct))
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
        .get_product_by_handle(&actor, query.currency.as_deref(), &path.handle)
        .await?;
    Ok(ApiResponse::ok(product_data(product)))
}

fn product_data(product: StorefrontCatalogProduct) -> StorefrontProductData {
    StorefrontProductData {
        id: product.id.as_uuid(),
        handle: product.handle,
        title: product.title,
        description: product.description,
        variants: product.variants.into_iter().map(variant_data).collect(),
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
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use chaos_application::ports::{ApiKeyMaterialGenerator, GeneratedApiKeyMaterial};
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        identity::UserId,
        merchant::{ApiKeyClass, ApiKeyId, ApiKeyMode, MerchantAccountId, SalesChannelId, StoreId},
    };
    use chaos_infrastructure::repositories::SecureApiKeyMaterialGenerator;
    use secrecy::ExposeSecret;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use tower::ServiceExt;

    use crate::http::{
        pricing::tests::{response_json, test_state},
        router,
    };

    use super::*;

    async fn insert_publishable_key(
        pool: &PgPool,
        account_id: MerchantAccountId,
        store_id: StoreId,
        user_id: UserId,
    ) -> GeneratedApiKeyMaterial {
        let material =
            SecureApiKeyMaterialGenerator.generate(ApiKeyClass::Publishable, ApiKeyMode::Live);
        let key_id = ApiKeyId::new();
        sqlx::query(
            "INSERT INTO merchant.api_keys \
             (id, merchant_account_id, store_id, key_identifier, secret_digest, \
              display_suffix, name, class, mode, created_by_user_id) \
             VALUES ($1, $2, $3, $4, $5, $6, 'Storefront HTTP', \
                     'publishable', 'live', $7)",
        )
        .bind(key_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(&material.key_identifier)
        .bind(material.secret_digest.as_slice())
        .bind(&material.display_suffix)
        .bind(user_id.as_uuid())
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.api_key_scopes (merchant_account_id, api_key_id, scope) \
             VALUES ($1, $2, 'catalog:read')",
        )
        .bind(account_id.as_uuid())
        .bind(key_id.as_uuid())
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
        let account_id = MerchantAccountId::new();
        let store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let price_list_id = Uuid::now_v7();
        let suffix = Uuid::now_v7().simple().to_string();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id.as_uuid())
            .bind(format!("storefront-http-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Storefront HTTP')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("storefront-http-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.stores \
             (id, merchant_account_id, code, name, status) \
             VALUES ($1, $2, 'storefront-http', 'Storefront HTTP', 'active')",
        )
        .bind(store_id.as_uuid())
        .bind(account_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.sales_channels \
             (id, merchant_account_id, store_id, code, name, kind, is_default) \
             VALUES ($1, $2, $3, 'web', 'Web', 'web', true)",
        )
        .bind(channel_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.store_currencies (merchant_account_id, store_id, currency) \
             VALUES ($1, $2, 'USD')",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.products \
             (id, merchant_account_id, store_id, handle, title, description, status) \
             VALUES ($1, $2, $3, 'public-shirt', 'Public Shirt', 'Public description', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, merchant_account_id, store_id, product_id, title, sku, status) \
             VALUES ($1, $2, $3, $4, 'Default', 'PUBLIC-SHIRT', 'active')",
        )
        .bind(variant_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_publications \
             (merchant_account_id, store_id, product_id, sales_channel_id) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(channel_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.price_lists \
             (id, merchant_account_id, store_id, code, name, currency, status) \
             VALUES ($1, $2, $3, 'public-retail', 'Public Retail', 'USD', 'active')",
        )
        .bind(price_list_id)
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.prices \
             (id, merchant_account_id, store_id, price_list_id, product_variant_id, amount_minor) \
             VALUES ($1, $2, $3, $4, $5, 4200)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(price_list_id)
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        let material = insert_publishable_key(&owner_pool, account_id, store_id, user_id).await;
        let state = test_state(&database_url, user_id);
        assert!(
            state
                .search_indexer
                .run_batch(Uuid::now_v7(), 100, state.clock.now())
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
