//! Storefront catalog (products) and product review endpoints.

use axum::{
    Router,
    extract::State,
    routing::{get, post},
};
use chaos_core::{
    catalog::SubmitReviewInput,
    contracts::{
        ReviewPageCursor, ReviewSummary, StorefrontCatalogProduct, StorefrontCatalogVariant,
        StorefrontProductCollection, StorefrontProductOption, StorefrontProductOptionValue,
        StorefrontRatingSummary, StorefrontSelectedOption,
    },
};
use chaos_domain::catalog::{MediaAssetStatus, ProductId, ReviewId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::shared::pagination::{
    CursorKind, decode_cursor, decode_review_cursor, encode_cursor, encode_review_cursor,
    page_limit, page_meta,
};
use crate::http::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, PublishableChannel,
};

use super::wire::{MediaResponse, ReviewStatus};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/products", get(list_products))
        .route("/products/{handle}", get(get_product))
        .route("/products/{product_id}/reviews", post(submit_review).get(list_reviews))
}

// ===== request contracts =====

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

/// Shared by `POST` and `GET /products/{product_id}/reviews`.
#[derive(Deserialize)]
struct ReviewProductPath {
    product_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitReviewRequest {
    rating: u8,
    #[serde(default)]
    title: Option<String>,
    content: String,
    author_name: String,
    #[serde(default)]
    author_email: Option<String>,
}

#[derive(Deserialize)]
struct ReviewListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

// ===== response contracts =====

#[derive(Serialize)]
struct ProductVariantResponse {
    id: Uuid,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sku: Option<String>,
    track_inventory: bool,
    available_quantity: i64,
    price: PriceResponse,
    selected_options: Vec<SelectedOptionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

impl From<StorefrontCatalogVariant> for ProductVariantResponse {
    fn from(variant: StorefrontCatalogVariant) -> Self {
        Self {
            id: variant.id.as_uuid(),
            title: variant.title,
            sku: variant.sku,
            track_inventory: variant.track_inventory,
            available_quantity: variant.available_quantity,
            price: PriceResponse {
                amount_minor: variant.amount_minor,
                currency: variant.currency.as_str().to_owned(),
            },
            selected_options: variant
                .selected_options
                .into_iter()
                .map(Into::into)
                .collect(),
            metadata: variant.metadata,
        }
    }
}

#[derive(Serialize)]
struct SelectedOptionResponse {
    option_id: Uuid,
    option_value_id: Uuid,
}

impl From<StorefrontSelectedOption> for SelectedOptionResponse {
    fn from(selection: StorefrontSelectedOption) -> Self {
        Self {
            option_id: selection.option_id.as_uuid(),
            option_value_id: selection.option_value_id.as_uuid(),
        }
    }
}

#[derive(Serialize)]
struct ProductOptionValueResponse {
    id: Uuid,
    value: String,
    position: u16,
}

impl From<StorefrontProductOptionValue> for ProductOptionValueResponse {
    fn from(value: StorefrontProductOptionValue) -> Self {
        Self {
            id: value.id.as_uuid(),
            value: value.value,
            position: value.position,
        }
    }
}

#[derive(Serialize)]
struct ProductOptionResponse {
    id: Uuid,
    name: String,
    position: u16,
    values: Vec<ProductOptionValueResponse>,
}

impl From<StorefrontProductOption> for ProductOptionResponse {
    fn from(option: StorefrontProductOption) -> Self {
        Self {
            id: option.id.as_uuid(),
            name: option.name,
            position: option.position,
            values: option.values.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Serialize)]
struct PriceResponse {
    amount_minor: i64,
    currency: String,
}

#[derive(Serialize)]
struct ProductResponse {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    options: Vec<ProductOptionResponse>,
    variants: Vec<ProductVariantResponse>,
    media: Vec<MediaResponse>,
    collections: Vec<ProductCollectionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating: Option<RatingResponse>,
}

impl From<StorefrontCatalogProduct> for ProductResponse {
    fn from(product: StorefrontCatalogProduct) -> Self {
        Self {
            id: product.id.as_uuid(),
            handle: product.handle,
            title: product.title,
            description: product.description,
            options: product.options.into_iter().map(Into::into).collect(),
            variants: product.variants.into_iter().map(Into::into).collect(),
            media: product.media.into_iter().map(Into::into).collect(),
            collections: product.collections.into_iter().map(Into::into).collect(),
            metadata: product.metadata,
            rating: product.rating.map(Into::into),
        }
    }
}

#[derive(Serialize)]
struct RatingResponse {
    average: f64,
    count: i64,
}

impl From<StorefrontRatingSummary> for RatingResponse {
    fn from(rating: StorefrontRatingSummary) -> Self {
        Self {
            average: rating.average,
            count: rating.count,
        }
    }
}

#[derive(Serialize)]
struct ProductCollectionResponse {
    id: Uuid,
    handle: String,
    title: String,
}

impl From<StorefrontProductCollection> for ProductCollectionResponse {
    fn from(collection: StorefrontProductCollection) -> Self {
        Self {
            id: collection.id.as_uuid(),
            handle: collection.handle,
            title: collection.title,
        }
    }
}

#[derive(Serialize)]
struct MutationResponse {
    id: Uuid,
}

#[derive(Serialize)]
struct ReviewResponse {
    id: Uuid,
    product_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<Uuid>,
    author_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    content: String,
    images: Vec<String>,
    status: ReviewStatus,
    is_staff_reply: bool,
    verified_buyer: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewed_at: Option<ApiDateTime>,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    replies: Vec<ReviewResponse>,
}

impl From<ReviewSummary> for ReviewResponse {
    fn from(review: ReviewSummary) -> Self {
        Self {
            id: review.id.as_uuid(),
            product_id: review.product_id.as_uuid(),
            parent_id: review.parent_review_id.map(ReviewId::as_uuid),
            author_name: review.author_name,
            rating: review.rating,
            title: review.title,
            content: review.content,
            images: review
                .images
                .into_iter()
                .filter(|image| image.status == MediaAssetStatus::Ready)
                .filter_map(|image| image.public_url)
                .collect(),
            status: review.status.into(),
            is_staff_reply: review.is_staff_reply,
            verified_buyer: review.verified_buyer,
            reviewed_at: review.reviewed_at.map(Into::into),
            created_at: review.created_at.into(),
            updated_at: review.updated_at.into(),
            replies: Vec::new(),
        }
    }
}

// ===== GET /products =====

async fn list_products(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiQuery(query): ApiQuery<CatalogQuery>,
) -> Result<ApiResponse<Vec<ProductResponse>>, ApiError> {
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
        ApiResponse::ok(page.items.into_iter().map(Into::into).collect())
            .with_meta(page_meta(page.has_more, next_cursor.flatten())),
    )
}

// ===== GET /products/{handle} =====

async fn get_product(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiPath(path): ApiPath<ProductPath>,
    ApiQuery(query): ApiQuery<ProductQuery>,
) -> Result<ApiResponse<ProductResponse>, ApiError> {
    let product = state
        .storefront_catalog
        .get_product_by_handle(&actor, query.currency.as_deref(), &path.handle)
        .await?;
    Ok(ApiResponse::ok(product.into()))
}

// ===== POST /products/{product_id}/reviews =====

async fn submit_review(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiPath(path): ApiPath<ReviewProductPath>,
    ApiJson(request): ApiJson<SubmitReviewRequest>,
) -> Result<ApiResponse<MutationResponse>, ApiError> {
    let id = state
        .review_administration
        .submit(SubmitReviewInput {
            actor,
            product_id: ProductId::from_uuid(path.product_id),
            rating: request.rating,
            title: request.title,
            content: request.content,
            author_name: request.author_name,
            author_email: request.author_email,
            now: state.clock.now(),
        })
        .await?;
    Ok(ApiResponse::created(MutationResponse { id: id.as_uuid() }))
}

// ===== GET /products/{product_id}/reviews =====

async fn list_reviews(
    State(state): State<ApiState>,
    PublishableChannel(actor): PublishableChannel,
    ApiPath(path): ApiPath<ReviewProductPath>,
    ApiQuery(query): ApiQuery<ReviewListQuery>,
) -> Result<ApiResponse<Vec<ReviewResponse>>, ApiError> {
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(decode_review_cursor)
        .transpose()?;
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
                .map(|item| {
                    encode_review_cursor(ReviewPageCursor {
                        sort_at: item.reviewed_at.unwrap_or(item.created_at),
                        id: item.id,
                    })
                })
        })
        .flatten();
    Ok(ApiResponse::ok(nest_replies(page.items)).with_meta(page_meta(page.has_more, next_cursor)))
}

fn nest_replies(items: Vec<ReviewSummary>) -> Vec<ReviewResponse> {
    let mut reviews: Vec<ReviewResponse> = Vec::new();
    for item in items {
        let is_reply = item.parent_review_id.is_some();
        let response = item.into();
        if is_reply {
            if let Some(parent) = reviews.last_mut() {
                parent.replies.push(response);
            }
        } else {
            reviews.push(response);
        }
    }
    reviews
}
