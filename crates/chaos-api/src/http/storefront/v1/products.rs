use axum::{
    Router,
    extract::State,
    routing::{get, post},
};
use chaos_core::{
    catalog::SubmitReviewInput,
    contracts::{
        ReviewSummary, StorefrontCatalogProduct, StorefrontCatalogVariant, StorefrontMediaAsset,
        StorefrontProductCollection, StorefrontProductOption, StorefrontProductOptionValue,
        StorefrontSelectedOption,
    },
};
use chaos_domain::catalog::{ProductId, ReviewId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::shared::pagination::{
    CursorKind, decode_cursor, encode_cursor, page_limit, page_meta,
};
use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, StorefrontMachine,
};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/products", get(list_products))
        .route("/products/{handle}", get(get_product))
        .route("/products/{product_id}/reviews", post(submit_review).get(list_product_reviews))
}

#[derive(Deserialize)]
struct CatalogQuery {
    currency: Option<String>,
    q: Option<String>,
    collection: Option<String>,
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
    track_inventory: bool,
    available_quantity: i64,
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
}

#[derive(Serialize)]
struct StorefrontProductData {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
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
        track_inventory: variant.track_inventory,
        available_quantity: variant.available_quantity,
        price: StorefrontPriceData {
            amount_minor: variant.amount_minor,
            currency: variant.currency.as_str().to_owned(),
        },
        selected_options: variant
            .selected_options
            .into_iter()
            .map(selected_option_data)
            .collect(),
        metadata: variant.metadata,
    }
}

#[derive(Deserialize)]
struct ReviewProductPath {
    product_id: Uuid,
}

#[derive(Deserialize)]
struct StorefrontListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubmitReviewBody {
    rating: u8,
    #[serde(default)]
    title: Option<String>,
    content: String,
    author_name: String,
    #[serde(default)]
    author_email: Option<String>,
}

#[derive(Serialize)]
struct MutationData {
    id: Uuid,
}

#[derive(Serialize)]
struct ReviewData {
    id: Uuid,
    product_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<Uuid>,
    author_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    author_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    content: String,
    images: Vec<String>,
    status: &'static str,
    is_staff_reply: bool,
    verified_buyer: bool,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    replies: Vec<ReviewData>,
}

async fn submit_review(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiPath(path): ApiPath<ReviewProductPath>,
    ApiJson(body): ApiJson<SubmitReviewBody>,
) -> Result<ApiResponse<MutationData>, ApiError> {
    let id = state
        .review_administration
        .submit(SubmitReviewInput {
            actor,
            product_id: ProductId::from_uuid(path.product_id),
            rating: body.rating,
            title: body.title,
            content: body.content,
            author_name: body.author_name,
            author_email: body.author_email,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(MutationData { id: id.as_uuid() }))
}

async fn list_product_reviews(
    State(state): State<ApiState>,
    StorefrontMachine(actor): StorefrontMachine,
    ApiPath(path): ApiPath<ReviewProductPath>,
    ApiQuery(query): ApiQuery<StorefrontListQuery>,
) -> Result<ApiResponse<Vec<ReviewData>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|value| decode_cursor(value, CursorKind::Review))
        .transpose()?
        .map(ReviewId::from_uuid);
    let page = state
        .storefront_reviews
        .list_for_product(&actor, ProductId::from_uuid(path.product_id), after, limit)
        .await?;
    let next_cursor = page
        .has_more
        .then(|| {
            page.items
                .iter()
                .rev()
                .find(|item| item.parent_review_id.is_none())
                .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::Review))
        })
        .flatten();
    Ok(ApiResponse::ok(nest_replies(page.items)).with_meta(page_meta(page.has_more, next_cursor)))
}

fn nest_replies(items: Vec<ReviewSummary>) -> Vec<ReviewData> {
    let mut result: Vec<ReviewData> = Vec::new();
    for item in items {
        let is_reply = item.parent_review_id.is_some();
        let data = review_data(item);
        if is_reply {
            if let Some(parent) = result.last_mut() {
                parent.replies.push(data);
            }
        } else {
            result.push(data);
        }
    }
    result
}

fn review_data(item: ReviewSummary) -> ReviewData {
    ReviewData {
        id: item.id.as_uuid(),
        product_id: item.product_id.as_uuid(),
        parent_id: item.parent_review_id.map(ReviewId::as_uuid),
        author_name: item.author_name,
        author_email: item.author_email,
        rating: item.rating,
        title: item.title,
        content: item.content,
        images: Vec::new(),
        status: item.status.as_str(),
        is_staff_reply: item.is_staff_reply,
        verified_buyer: item.verified_buyer,
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
        replies: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use chaos_core::adapters::postgres::DefaultPublishableKeyGenerator;
    use chaos_core::contracts::GeneratedPublishableKey;
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        identity::UserId,
        store::{PublishableKeyId, SalesChannelId, StoreId},
    };
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use tower::ServiceExt;

    use crate::http::{
        router,
        shared::test_support::{response_json, test_state},
    };

    use super::*;

    async fn insert_publishable_key(pool: &PgPool, store_id: StoreId) -> GeneratedPublishableKey {
        let material = DefaultPublishableKeyGenerator.generate();
        let key_id = PublishableKeyId::new();
        sqlx::query(
            "INSERT INTO commerce.store_publishable_keys \
             (id, store_id, public_key, name) \
             VALUES ($1, $2, $3, 'Storefront HTTP')",
        )
        .bind(key_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(&material.public_key)
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
            "INSERT INTO commerce.store_sales_channels \
             (id, store_id, code, name, storefront_origin, is_default) \
             VALUES ($1, $2, 'web', 'Web', $3, true)",
        )
        .bind(channel_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(format!("https://{suffix}.storefront.example.test"))
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
        let material = insert_publishable_key(&owner_pool, store_id).await;
        let state = test_state(&database_url, user_id);
        assert!(
            chaos_core::adapters::postgres::PostgresSearchIndexer::new(
                state.infrastructure.runtime_pool(),
            )
            .run_batch(100, state.clock.now())
            .await
            .unwrap()
                >= 2
        );
        let authorize = format!("Bearer {}", material.public_key);

        let response = router(state.clone())
            .oneshot(
                Request::get("/storefront/v1/products")
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
                Request::get("/storefront/v1/products/public-shirt")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router(state.clone())
            .oneshot(
                Request::get("/storefront/v1/products?currency=usd")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = router(state)
            .oneshot(
                Request::get("/storefront/v1/products/missing-product")
                    .header("authorization", &authorize)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
