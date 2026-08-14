use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
};
use chaos_application::{
    ApplicationError,
    catalog::{
        CreateProductInput, CreateProductOptionInput, CreateProductSelectedOptionInput,
        CreateProductVariantInput,
    },
    ports::IdempotencyRequest,
};
use chaos_domain::{
    catalog::ProductId,
    merchant::{MerchantAccountId, StoreId},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, MerchantContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
    response::format_time,
};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/products",
            post(create_product).get(list_products),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/products/{product_id}",
            get(get_product),
        )
        .layer(DefaultBodyLimit::max(256 * 1024))
}

#[derive(Deserialize)]
struct ProductPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
struct ProductDetailPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    product_id: Uuid,
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
struct ProductListData {
    id: Uuid,
    handle: String,
    title: String,
    status: &'static str,
    variant_count: u32,
    created_at: String,
    updated_at: String,
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
    created_at: String,
    updated_at: String,
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
    created_at: String,
    updated_at: String,
}

async fn create_product(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ProductPath>,
    ApiJson(body): ApiJson<CreateProductBody>,
) -> Result<ApiResponse<ProductCreatedData>, ApiError> {
    let path_account_id = MerchantAccountId::from_uuid(path.merchant_account_id);
    if path_account_id != actor.merchant_account_id() {
        return Err(ApplicationError::Forbidden.into());
    }
    let idempotency_key = idempotency_key(&headers)?;
    let request_fingerprint = Sha256::digest(
        serde_json::to_vec(&(path.store_id, &body))
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .into();
    let output = state
        .create_product
        .execute(CreateProductInput {
            actor,
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
                })
                .collect(),
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
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ProductPath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<ProductListData>>, ApiError> {
    ensure_account_path(actor.merchant_account_id(), path.merchant_account_id)?;
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::Product))
        .transpose()?
        .map(ProductId::from_uuid);
    let page = state
        .catalog_queries
        .list_products(actor, StoreId::from_uuid(path.store_id), after, limit)
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
                created_at: format_time(item.created_at)?,
                updated_at: format_time(item.updated_at)?,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor.flatten())))
}

async fn get_product(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<ProductDetailPath>,
) -> Result<ApiResponse<ProductDetailData>, ApiError> {
    ensure_account_path(actor.merchant_account_id(), path.merchant_account_id)?;
    let product = state
        .catalog_queries
        .get_product(
            actor,
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
                created_at: format_time(variant.created_at)?,
                updated_at: format_time(variant.updated_at)?,
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
        created_at: format_time(product.created_at)?,
        updated_at: format_time(product.updated_at)?,
    }))
}

fn ensure_account_path(actual: MerchantAccountId, extracted: Uuid) -> Result<(), ApiError> {
    if actual.as_uuid() == extracted {
        return Ok(());
    }
    Err(ApplicationError::Forbidden.into())
}

const fn enabled() -> bool {
    true
}
