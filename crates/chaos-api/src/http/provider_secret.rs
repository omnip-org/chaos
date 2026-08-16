use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, header::CACHE_CONTROL},
    routing::post,
};
use chaos_application::{
    ApplicationError, merchant::CreateProviderSecretInput, ports::ProviderSecretKind,
};
use chaos_domain::merchant::StoreId;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, ApiJson, ApiPath, ApiResponse, ApiState, MerchantContext};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/merchant-accounts/{merchant_account_id}/stores/{store_id}/provider-secrets",
            post(create_provider_secret),
        )
        .layer(DefaultBodyLimit::max(20 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
    merchant_account_id: Uuid,
    store_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProviderSecretBody {
    kind: String,
    value: String,
}

#[derive(Serialize)]
struct ProviderSecretCreatedData {
    secret_reference: String,
}

async fn create_provider_secret(
    State(state): State<ApiState>,
    MerchantContext(actor): MerchantContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreateProviderSecretBody>,
) -> Result<(HeaderMap, ApiResponse<ProviderSecretCreatedData>), ApiError> {
    if actor.merchant_account_id().as_uuid() != path.merchant_account_id {
        return Err(ApplicationError::Forbidden.into());
    }
    let kind = ProviderSecretKind::parse(&body.kind).ok_or_else(|| {
        ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "kind",
                reason: "must be payment_credential, payment_webhook, shipping_credential, or analytics_credential".into(),
            }],
        }
    })?;
    let secret_reference = state
        .provider_secret_management
        .create(CreateProviderSecretInput {
            actor,
            store_id: StoreId::from_uuid(path.store_id),
            kind,
            value: SecretString::from(body.value),
        })
        .await?;
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((
        headers,
        ApiResponse::created(ProviderSecretCreatedData { secret_reference }),
    ))
}
