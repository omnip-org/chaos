use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::{
    ApplicationError,
    catalog::{
        CreateProductInput, CreateProductOptionInput, CreateProductSelectedOptionInput,
        CreateProductVariantInput,
    },
    ports::IdempotencyRequest,
};
use chaos_domain::merchant::{MerchantAccountId, StoreId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ApiError, ApiJson, ApiPath, ApiResponse, ApiState, MerchantContext, merchant::idempotency_key,
};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/products",
            post(create_product),
        )
        .layer(DefaultBodyLimit::max(256 * 1024))
}

#[derive(Deserialize)]
struct ProductPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
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

const fn enabled() -> bool {
    true
}
