use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::{
    ApplicationError,
    ports::IdempotencyRequest,
    pricing::{CreatePriceInput, CreatePriceListInput},
};
use chaos_domain::{FieldViolation, catalog::ProductVariantId, merchant::StoreId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use super::{
    ApiError, ApiJson, ApiPath, ApiResponse, ApiState, MerchantContext, merchant::idempotency_key,
};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/price-lists",
            post(create_price_list),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreatePriceListBody {
    code: String,
    name: String,
    currency: String,
    #[serde(default)]
    tax_inclusive: bool,
    starts_at: Option<String>,
    ends_at: Option<String>,
    #[serde(default)]
    activate: bool,
    #[serde(default)]
    prices: Vec<CreatePriceBody>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreatePriceBody {
    product_variant_id: Uuid,
    amount_minor: i64,
}

#[derive(Serialize)]
struct PriceListCreatedData {
    id: Uuid,
}

async fn create_price_list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreatePriceListBody>,
) -> Result<ApiResponse<PriceListCreatedData>, ApiError> {
    if actor.merchant_account_id().as_uuid() != path.merchant_account_id {
        return Err(ApplicationError::Forbidden.into());
    }
    let idempotency_key = idempotency_key(&headers)?;
    let request_fingerprint = Sha256::digest(
        serde_json::to_vec(&(path.store_id, &body))
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .into();
    let starts_at = parse_optional_time("starts_at", body.starts_at.as_deref())?;
    let ends_at = parse_optional_time("ends_at", body.ends_at.as_deref())?;
    let output = state
        .create_price_list
        .execute(CreatePriceListInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            code: body.code,
            name: body.name,
            currency: body.currency,
            tax_inclusive: body.tax_inclusive,
            starts_at,
            ends_at,
            activate: body.activate,
            prices: body
                .prices
                .into_iter()
                .map(|price| CreatePriceInput {
                    product_variant_id: ProductVariantId::from_uuid(price.product_variant_id),
                    amount_minor: price.amount_minor,
                })
                .collect(),
            idempotency: IdempotencyRequest {
                key: idempotency_key,
                request_fingerprint,
            },
        })
        .await?;
    Ok(ApiResponse::created(PriceListCreatedData {
        id: output.price_list_id.as_uuid(),
    }))
}

fn parse_optional_time(
    field: &'static str,
    value: Option<&str>,
) -> Result<Option<OffsetDateTime>, ApiError> {
    value
        .map(|value| {
            OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
                ApplicationError::Validation {
                    violations: vec![FieldViolation {
                        field,
                        reason: "must be an RFC 3339 timestamp".into(),
                    }],
                }
                .into()
            })
        })
        .transpose()
}
