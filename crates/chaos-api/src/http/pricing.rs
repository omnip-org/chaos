use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::post,
};
use chaos_application::{
    ApplicationError,
    ports::IdempotencyRequest,
    pricing::{
        ChangePriceListStatusInput, ChangePromotionStatusInput, ChangeTaxRuleStatusInput,
        CreatePriceInput, CreatePriceListInput, CreatePromotionInput, CreateTaxRuleInput,
        UpdatePriceListInput,
    },
};
use chaos_domain::{
    CurrencyCode, FieldViolation,
    catalog::ProductVariantId,
    merchant::StoreId,
    pricing::{
        PriceListId, PromotionId, PromotionStatus, PromotionTrigger, PromotionValue, TaxRuleId,
        TaxRuleStatus,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    ApiDateTime, ApiError, ApiJson, ApiPath, ApiQuery, ApiResponse, ApiState, MerchantContext,
    merchant::{CursorKind, decode_cursor, encode_cursor, idempotency_key, page_limit, page_meta},
    response::parse_api_time,
};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/price-lists",
            post(create_price_list).get(list_price_lists),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/price-lists/{price_list_id}",
            axum::routing::get(get_price_list).put(update_price_list),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/price-lists/{price_list_id}/activate",
            post(activate_price_list),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/price-lists/{price_list_id}/archive",
            post(archive_price_list),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/tax-rules",
            post(create_tax_rule).get(list_tax_rules),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/tax-rules/{tax_rule_id}/{operation}",
            post(change_tax_rule_status),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/promotions",
            post(create_promotion).get(list_promotions),
        )
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/promotions/{promotion_id}/{operation}",
            post(change_promotion_status),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
struct PriceListPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    price_list_id: Uuid,
}

#[derive(Deserialize)]
struct TaxRulePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    tax_rule_id: Uuid,
    operation: String,
}

#[derive(Deserialize)]
struct PromotionPath {
    merchant_account_id: Uuid,
    store_id: Uuid,
    promotion_id: Uuid,
    operation: String,
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u16>,
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UpdatePriceListBody {
    code: String,
    name: String,
    currency: String,
    #[serde(default)]
    tax_inclusive: bool,
    starts_at: Option<String>,
    ends_at: Option<String>,
    #[serde(default)]
    prices: Vec<CreatePriceBody>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateTaxRuleBody {
    code: String,
    name: String,
    country_code: String,
    rate_basis_points: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreatePromotionBody {
    handle: String,
    name: String,
    trigger: String,
    redemption_code: Option<String>,
    value_kind: String,
    rate_basis_points: Option<u32>,
    amount_minor: Option<i64>,
    maximum_amount_minor: Option<i64>,
    currency: String,
    #[serde(default)]
    minimum_subtotal_amount_minor: i64,
    #[serde(default = "default_promotion_priority")]
    priority: u16,
    starts_at: Option<String>,
    ends_at: Option<String>,
}

const fn default_promotion_priority() -> u16 {
    100
}

#[derive(Serialize)]
struct PriceListCreatedData {
    id: Uuid,
}

#[derive(Serialize)]
struct PriceListMutationData {
    id: Uuid,
}

#[derive(Serialize)]
struct PriceListData {
    id: Uuid,
    code: String,
    name: String,
    currency: String,
    tax_inclusive: bool,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    starts_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ends_at: Option<ApiDateTime>,
    price_count: u32,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct PriceData {
    product_variant_id: Uuid,
    amount_minor: i64,
}

#[derive(Serialize)]
struct PriceListDetailData {
    #[serde(flatten)]
    price_list: PriceListData,
    prices: Vec<PriceData>,
}

#[derive(Serialize)]
struct TaxRuleData {
    id: Uuid,
    code: String,
    name: String,
    country_code: String,
    rate_basis_points: u32,
    status: &'static str,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

#[derive(Serialize)]
struct PromotionData {
    id: Uuid,
    handle: String,
    name: String,
    trigger: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    redemption_code: Option<String>,
    value_kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_basis_points: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum_amount_minor: Option<i64>,
    currency: String,
    minimum_subtotal_amount_minor: i64,
    priority: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    starts_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ends_at: Option<ApiDateTime>,
    status: &'static str,
    created_at: ApiDateTime,
    updated_at: ApiDateTime,
}

async fn create_promotion(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreatePromotionBody>,
) -> Result<ApiResponse<PromotionData>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let trigger = PromotionTrigger::parse(&body.trigger).ok_or_else(|| invalid_enum("trigger"))?;
    let value = promotion_value(&body)?;
    let currency = CurrencyCode::parse(&body.currency).map_err(ApplicationError::from)?;
    let starts_at = parse_optional_time("starts_at", body.starts_at.as_deref())?;
    let ends_at = parse_optional_time("ends_at", body.ends_at.as_deref())?;
    let idempotency = collection_mutation_request(&headers, path.store_id, &body)?;
    let detail = state
        .promotion_management
        .create(CreatePromotionInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            handle: body.handle,
            name: body.name,
            trigger,
            redemption_code: body.redemption_code,
            value,
            currency,
            minimum_subtotal_amount_minor: body.minimum_subtotal_amount_minor,
            priority: body.priority,
            starts_at,
            ends_at,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(promotion_data(detail)))
}

async fn list_promotions(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<Vec<PromotionData>>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let values = state
        .promotion_management
        .list(actor, StoreId::from_uuid(path.store_id))
        .await?;
    Ok(ApiResponse::ok(
        values.into_iter().map(promotion_data).collect(),
    ))
}

async fn change_promotion_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<PromotionPath>,
) -> Result<ApiResponse<PromotionData>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let status = match path.operation.as_str() {
        "activate" => PromotionStatus::Active,
        "archive" => PromotionStatus::Archived,
        _ => {
            return Err(ApplicationError::NotFound {
                resource: "operation",
                id: path.operation,
            }
            .into());
        }
    };
    let idempotency =
        mutation_request(&headers, path.store_id, path.promotion_id, &status.as_str())?;
    let detail = state
        .promotion_management
        .change_status(ChangePromotionStatusInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            promotion_id: PromotionId::from_uuid(path.promotion_id),
            status,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(promotion_data(detail)))
}

fn promotion_value(body: &CreatePromotionBody) -> Result<PromotionValue, ApiError> {
    match body.value_kind.as_str() {
        "percentage" if body.amount_minor.is_none() => Ok(PromotionValue::Percentage {
            rate_basis_points: body
                .rate_basis_points
                .ok_or_else(|| invalid_enum("rate_basis_points"))?,
            maximum_amount_minor: body.maximum_amount_minor,
        }),
        "fixed_amount"
            if body.rate_basis_points.is_none() && body.maximum_amount_minor.is_none() =>
        {
            Ok(PromotionValue::FixedAmount {
                amount_minor: body
                    .amount_minor
                    .ok_or_else(|| invalid_enum("amount_minor"))?,
            })
        }
        _ => Err(invalid_enum("value_kind").into()),
    }
}

fn promotion_data(detail: chaos_application::ports::PromotionDetail) -> PromotionData {
    let promotion = detail.promotion;
    PromotionData {
        id: promotion.id().as_uuid(),
        handle: promotion.handle().into(),
        name: promotion.name().into(),
        trigger: promotion.trigger().as_str(),
        redemption_code: promotion.redemption_code().map(Into::into),
        value_kind: promotion.value().kind(),
        rate_basis_points: promotion.value().rate_basis_points(),
        amount_minor: promotion.value().amount_minor(),
        maximum_amount_minor: promotion.value().maximum_amount_minor(),
        currency: promotion.currency().as_str().into(),
        minimum_subtotal_amount_minor: promotion.minimum_subtotal_amount_minor(),
        priority: promotion.priority(),
        starts_at: promotion.starts_at().map(Into::into),
        ends_at: promotion.ends_at().map(Into::into),
        status: promotion.status().as_str(),
        created_at: detail.created_at.into(),
        updated_at: detail.updated_at.into(),
    }
}

fn invalid_enum(field: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: "has an invalid or incompatible value".into(),
        }],
    }
}

async fn create_tax_rule(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreateTaxRuleBody>,
) -> Result<ApiResponse<TaxRuleData>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let idempotency = collection_mutation_request(&headers, path.store_id, &body)?;
    let detail = state
        .tax_management
        .create(CreateTaxRuleInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            code: body.code,
            name: body.name,
            country_code: body.country_code,
            rate_basis_points: body.rate_basis_points,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::created(tax_rule_data(detail)))
}

async fn list_tax_rules(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
) -> Result<ApiResponse<Vec<TaxRuleData>>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let rules = state
        .tax_management
        .list(actor, StoreId::from_uuid(path.store_id))
        .await?;
    Ok(ApiResponse::ok(
        rules.into_iter().map(tax_rule_data).collect(),
    ))
}

async fn change_tax_rule_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<TaxRulePath>,
) -> Result<ApiResponse<TaxRuleData>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let status = match path.operation.as_str() {
        "activate" => TaxRuleStatus::Active,
        "archive" => TaxRuleStatus::Archived,
        _ => {
            return Err(ApplicationError::NotFound {
                resource: "operation",
                id: path.operation,
            }
            .into());
        }
    };
    let idempotency =
        mutation_request(&headers, path.store_id, path.tax_rule_id, &status.as_str())?;
    let detail = state
        .tax_management
        .change_status(ChangeTaxRuleStatusInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            rule_id: TaxRuleId::from_uuid(path.tax_rule_id),
            status,
            idempotency,
        })
        .await?;
    Ok(ApiResponse::ok(tax_rule_data(detail)))
}

fn tax_rule_data(detail: chaos_application::ports::TaxRuleDetail) -> TaxRuleData {
    TaxRuleData {
        id: detail.rule.id().as_uuid(),
        code: detail.rule.code().into(),
        name: detail.rule.name().into(),
        country_code: detail.rule.country_code().into(),
        rate_basis_points: detail.rule.rate_basis_points(),
        status: detail.rule.status().as_str(),
        created_at: detail.created_at.into(),
        updated_at: detail.updated_at.into(),
    }
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

async fn list_price_lists(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<ApiResponse<Vec<PriceListData>>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let limit = page_limit(query.limit)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, CursorKind::PriceList))
        .transpose()?
        .map(PriceListId::from_uuid);
    let page = state
        .pricing_management
        .list(actor, StoreId::from_uuid(path.store_id), after, limit)
        .await?;
    let next_cursor = page.has_more.then(|| {
        page.items
            .last()
            .map(|item| encode_cursor(item.id.as_uuid(), CursorKind::PriceList))
    });
    let data = page
        .items
        .into_iter()
        .map(price_list_data)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ApiResponse::ok(data).with_meta(page_meta(page.has_more, next_cursor.flatten())))
}

async fn get_price_list(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<PriceListPath>,
) -> Result<ApiResponse<PriceListDetailData>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let detail = state
        .pricing_management
        .get(
            actor,
            StoreId::from_uuid(path.store_id),
            PriceListId::from_uuid(path.price_list_id),
        )
        .await?;
    Ok(ApiResponse::ok(PriceListDetailData {
        price_list: price_list_data(detail.item)?,
        prices: detail
            .prices
            .into_iter()
            .map(|price| PriceData {
                product_variant_id: price.product_variant_id.as_uuid(),
                amount_minor: price.amount_minor,
            })
            .collect(),
    }))
}

async fn update_price_list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<PriceListPath>,
    ApiJson(body): ApiJson<UpdatePriceListBody>,
) -> Result<ApiResponse<PriceListMutationData>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let request = mutation_request(&headers, path.store_id, path.price_list_id, &body)?;
    let id = state
        .pricing_management
        .update(UpdatePriceListInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            price_list_id: PriceListId::from_uuid(path.price_list_id),
            code: body.code,
            name: body.name,
            currency: body.currency,
            tax_inclusive: body.tax_inclusive,
            starts_at: parse_optional_time("starts_at", body.starts_at.as_deref())?,
            ends_at: parse_optional_time("ends_at", body.ends_at.as_deref())?,
            prices: body
                .prices
                .into_iter()
                .map(|price| CreatePriceInput {
                    product_variant_id: ProductVariantId::from_uuid(price.product_variant_id),
                    amount_minor: price.amount_minor,
                })
                .collect(),
            idempotency: request,
        })
        .await?;
    Ok(ApiResponse::ok(PriceListMutationData { id: id.as_uuid() }))
}

async fn activate_price_list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<PriceListPath>,
) -> Result<ApiResponse<PriceListMutationData>, ApiError> {
    change_status(state, headers, actor, path, true).await
}

async fn archive_price_list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<PriceListPath>,
) -> Result<ApiResponse<PriceListMutationData>, ApiError> {
    change_status(state, headers, actor, path, false).await
}

async fn change_status(
    state: ApiState,
    headers: HeaderMap,
    actor: chaos_application::merchant::MerchantActor,
    path: PriceListPath,
    activate: bool,
) -> Result<ApiResponse<PriceListMutationData>, ApiError> {
    ensure_account(
        actor.merchant_account_id().as_uuid(),
        path.merchant_account_id,
    )?;
    let idempotency = IdempotencyRequest {
        key: idempotency_key(&headers)?,
        request_fingerprint: Sha256::digest(
            format!("{}:{}:{}", path.store_id, path.price_list_id, activate).as_bytes(),
        )
        .into(),
    };
    let input = ChangePriceListStatusInput {
        actor,
        store_id: StoreId::from_uuid(path.store_id),
        price_list_id: PriceListId::from_uuid(path.price_list_id),
        idempotency,
    };
    let id = if activate {
        state.pricing_management.activate(input).await?
    } else {
        state.pricing_management.archive(input).await?
    };
    Ok(ApiResponse::ok(PriceListMutationData { id: id.as_uuid() }))
}

fn mutation_request<T: Serialize>(
    headers: &HeaderMap,
    store_id: Uuid,
    price_list_id: Uuid,
    body: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(store_id, price_list_id, body))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

fn collection_mutation_request<T: Serialize>(
    headers: &HeaderMap,
    store_id: Uuid,
    body: &T,
) -> Result<IdempotencyRequest, ApiError> {
    Ok(IdempotencyRequest {
        key: idempotency_key(headers)?,
        request_fingerprint: Sha256::digest(
            serde_json::to_vec(&(store_id, body))
                .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .into(),
    })
}

fn price_list_data(
    item: chaos_application::ports::PriceListReadItem,
) -> Result<PriceListData, ApplicationError> {
    Ok(PriceListData {
        id: item.id.as_uuid(),
        code: item.code,
        name: item.name,
        currency: item.currency.as_str().to_owned(),
        tax_inclusive: item.tax_inclusive,
        status: item.status.as_str(),
        starts_at: item.starts_at.map(Into::into),
        ends_at: item.ends_at.map(Into::into),
        price_count: item.price_count,
        created_at: item.created_at.into(),
        updated_at: item.updated_at.into(),
    })
}

fn ensure_account(actual: Uuid, expected: Uuid) -> Result<(), ApiError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden.into())
    }
}

fn parse_optional_time(
    field: &'static str,
    value: Option<&str>,
) -> Result<Option<OffsetDateTime>, ApiError> {
    value
        .map(|value| {
            parse_api_time(value).map_err(|_| {
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

#[cfg(test)]
pub(crate) mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode},
    };
    use chaos_application::{
        ApplicationError,
        ports::{CeremonyOptions, PasswordlessAuthentication, SessionGrant},
    };
    use chaos_domain::identity::{Email, UserId};
    use chaos_infrastructure::{config::Settings, state::AppState};
    use secrecy::SecretString;
    use serde_json::{Value, json};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use crate::{http::router, lifecycle::Lifecycle};

    use super::*;

    struct FixedSession(UserId);

    #[async_trait::async_trait]
    impl PasswordlessAuthentication for FixedSession {
        async fn authenticate_session(
            &self,
            _token: &SecretString,
        ) -> Result<UserId, ApplicationError> {
            Ok(self.0)
        }

        async fn request_magic_link(&self, _email: Email) -> Result<(), ApplicationError> {
            Err(not_used())
        }

        async fn consume_magic_link(
            &self,
            _token: &SecretString,
        ) -> Result<SessionGrant, ApplicationError> {
            Err(not_used())
        }

        async fn revoke_session(&self, _token: &SecretString) -> Result<(), ApplicationError> {
            Err(not_used())
        }

        async fn start_passkey_registration(
            &self,
            _user_id: UserId,
        ) -> Result<CeremonyOptions, ApplicationError> {
            Err(not_used())
        }

        async fn finish_passkey_registration(
            &self,
            _user_id: UserId,
            _ceremony_id: Uuid,
            _credential: Value,
            _name: &str,
        ) -> Result<Uuid, ApplicationError> {
            Err(not_used())
        }

        async fn start_passkey_authentication(
            &self,
            _email: Email,
        ) -> Result<CeremonyOptions, ApplicationError> {
            Err(not_used())
        }

        async fn finish_passkey_authentication(
            &self,
            _ceremony_id: Uuid,
            _credential: Value,
        ) -> Result<SessionGrant, ApplicationError> {
            Err(not_used())
        }
    }

    fn not_used() -> ApplicationError {
        ApplicationError::Unexpected(anyhow::anyhow!("unused authentication operation"))
    }

    pub(crate) fn test_state(database_url: &str, user_id: UserId) -> ApiState {
        let settings = Settings {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            database_url: database_url.into(),
            database_control_plane_url: database_url.into(),
            database_max_connections: 4,
            database_control_plane_max_connections: 1,
            database_acquire_timeout: Duration::from_secs(2),
            database_runtime_role: Some("chaos_runtime".into()),
            database_control_plane_role: None,
            redis_url: "redis://127.0.0.1:1".into(),
            webauthn_rp_id: "localhost".into(),
            webauthn_rp_origin: "http://localhost:8080".into(),
            auth_public_base_url: "http://localhost:8080".into(),
            smtp_url: "smtp://localhost:1025".into(),
            email_from: "Chaos <no-reply@localhost>".into(),
            resend_api_key: None,
            resend_webhook_secret: None,
            resend_api_base_url: "http://localhost:12112/".parse().unwrap(),
            payment_webhook_secret: "test-payment-webhook-secret-32-bytes".into(),
            stripe_api_base_url: "http://127.0.0.1:12111/".parse().unwrap(),
            shopper_token_active_key_id: "test".into(),
            shopper_token_active_secret: "test-shopper-token-secret-32-bytes".into(),
            shopper_token_previous_key: None,
            dependency_timeout: Duration::from_secs(1),
            shutdown_drain_delay: Duration::ZERO,
            shutdown_worker_timeout: Duration::from_secs(1),
            log_filter: "off".into(),
            log_json: false,
        };
        let mut state = ApiState::new(
            AppState::new(&settings).unwrap(),
            Lifecycle::new(),
            &settings,
        )
        .unwrap();
        state.passwordless_auth = Arc::new(FixedSession(user_id));
        state
    }

    pub(crate) async fn response_json(response: axum::response::Response) -> Value {
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    pub(crate) fn request(
        method: Method,
        uri: &str,
        idempotency_key: Option<&str>,
        body: Option<Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", "Bearer test-session");
        if let Some(key) = idempotency_key {
            builder = builder.header("idempotency-key", key);
        }
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        builder
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn price_list_http_matrix_covers_success_authorization_validation_conflict_and_not_found()
    {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let owner_id = UserId::new();
        let support_id = UserId::new();
        let account_id = chaos_domain::merchant::MerchantAccountId::new();
        let store_id = StoreId::new();
        let product_id = chaos_domain::catalog::ProductId::new();
        let variant_id = ProductVariantId::new();
        let price_list_id = PriceListId::new();
        let reserved_price_list_id = PriceListId::new();
        let suffix = Uuid::now_v7().simple().to_string();

        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(id.as_uuid())
                .bind(format!("pricing-http-{role}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Pricing HTTP Test')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("pricing-http-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, role) in [(owner_id, "owner"), (support_id, "support")] {
            sqlx::query(
                "INSERT INTO merchant.merchant_account_memberships \
                 (merchant_account_id, user_id, role) \
                 VALUES ($1, $2, $3::merchant.merchant_role)",
            )
            .bind(account_id.as_uuid())
            .bind(id.as_uuid())
            .bind(role)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.stores (id, merchant_account_id, code, name) \
             VALUES ($1, $2, 'pricing-http', 'Pricing HTTP')",
        )
        .bind(store_id.as_uuid())
        .bind(account_id.as_uuid())
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
             (id, merchant_account_id, store_id, handle, title, status) \
             VALUES ($1, $2, $3, 'http-shirt', 'HTTP Shirt', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, merchant_account_id, store_id, product_id, title, status) \
             VALUES ($1, $2, $3, $4, 'Default', 'active')",
        )
        .bind(variant_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, code) in [
            (price_list_id, "http-retail"),
            (reserved_price_list_id, "reserved-code"),
        ] {
            sqlx::query(
                "INSERT INTO pricing.price_lists \
                 (id, merchant_account_id, store_id, code, name, currency) \
                 VALUES ($1, $2, $3, $4, 'HTTP Retail', 'USD')",
            )
            .bind(id.as_uuid())
            .bind(account_id.as_uuid())
            .bind(store_id.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO pricing.prices \
             (id, merchant_account_id, store_id, price_list_id, product_variant_id, amount_minor) \
             VALUES ($1, $2, $3, $4, $5, 2500)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(price_list_id.as_uuid())
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let collection_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/price-lists",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let detail_uri = format!("{collection_uri}/{}", price_list_id.as_uuid());
        let owner_state = test_state(&database_url, owner_id);
        let tax_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/tax-rules",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &tax_uri,
                Some(&format!("create-tax-{suffix}")),
                Some(json!({
                    "code": "us-sales-tax",
                    "name": "US sales tax",
                    "country_code": "us",
                    "rate_basis_points": 825
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let tax_rule = response_json(response).await;
        let tax_rule_id = tax_rule["data"]["id"].as_str().unwrap();
        assert_eq!(tax_rule["data"]["country_code"], "US");

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &tax_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &format!("{tax_uri}/{tax_rule_id}/archive"),
                Some(&format!("archive-tax-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["status"], "archived");

        let promotion_uri = format!(
            "/admin/v1/merchant-accounts/{}/stores/{}/promotions",
            account_id.as_uuid(),
            store_id.as_uuid()
        );
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &promotion_uri,
                Some(&format!("create-promotion-{suffix}")),
                Some(json!({
                    "handle": "summer-ten", "name": "Summer ten percent",
                    "trigger": "code", "redemption_code": "save-10",
                    "value_kind": "percentage", "rate_basis_points": 1000,
                    "maximum_amount_minor": 500, "currency": "USD",
                    "minimum_subtotal_amount_minor": 1000, "priority": 10
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let promotion = response_json(response).await;
        let promotion_id = promotion["data"]["id"].as_str().unwrap();
        assert_eq!(promotion["data"]["redemption_code"], "SAVE-10");

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &promotion_uri,
                Some(&format!("duplicate-promotion-{suffix}")),
                Some(json!({
                    "handle": "other-summer", "name": "Other summer",
                    "trigger": "code", "redemption_code": "SAVE-10",
                    "value_kind": "fixed_amount", "amount_minor": 100, "currency": "USD"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &promotion_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &format!("{promotion_uri}/{promotion_id}/archive"),
                Some(&format!("archive-promotion-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["status"], "archived");

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &promotion_uri,
                Some(&format!("replace-promotion-{suffix}")),
                Some(json!({
                    "handle": "summer-ten-v2", "name": "Summer ten percent v2",
                    "trigger": "code", "redemption_code": "SAVE-10",
                    "value_kind": "percentage", "rate_basis_points": 1200, "currency": "USD"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let replacement_id = response_json(response).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &format!("{promotion_uri}/{promotion_id}/activate"),
                Some(&format!("conflicting-activation-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &format!("{promotion_uri}/{replacement_id}/archive"),
                Some(&format!("archive-replacement-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &format!("{promotion_uri}/{promotion_id}/activate"),
                Some(&format!("activate-promotion-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["status"], "active");

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &collection_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let response = router(owner_state.clone())
            .oneshot(request(Method::GET, &detail_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["data"]["prices"][0]["amount_minor"],
            2500
        );

        let activate_uri = format!("{detail_uri}/activate");
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::POST,
                &activate_uri,
                Some(&format!("activate-{suffix}")),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let update_body = json!({
            "code": "http-retail-updated",
            "name": "HTTP Retail Updated",
            "currency": "USD",
            "tax_inclusive": true,
            "prices": [{ "product_variant_id": variant_id.as_uuid(), "amount_minor": 3000 }]
        });
        let response = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &detail_uri,
                Some(&format!("update-{suffix}")),
                Some(update_body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &detail_uri,
                Some(&format!("validation-{suffix}")),
                Some(json!({ "code": "valid-code", "name": "Invalid", "currency": "usd" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "validation_failed"
        );

        let response = router(owner_state.clone())
            .oneshot(request(
                Method::PUT,
                &detail_uri,
                Some(&format!("conflict-{suffix}")),
                Some(json!({
                    "code": "reserved-code",
                    "name": "Conflict",
                    "currency": "USD",
                    "prices": [{ "product_variant_id": variant_id.as_uuid(), "amount_minor": 3000 }]
                })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "price_list_code_taken"
        );

        let missing_uri = format!("{collection_uri}/{}", Uuid::now_v7());
        let response = router(owner_state)
            .oneshot(request(Method::GET, &missing_uri, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let support_state = test_state(&database_url, support_id);
        let response = router(support_state)
            .oneshot(request(
                Method::PUT,
                &detail_uri,
                Some(&format!("forbidden-{suffix}")),
                Some(json!({ "code": "support-write", "name": "No", "currency": "USD" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
