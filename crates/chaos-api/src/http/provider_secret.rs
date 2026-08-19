use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, header::CACHE_CONTROL},
    routing::post,
};
use chaos_application::{
    ApplicationError,
    merchant::CreateProviderSecretInput,
    ports::{AdminActor, ProviderSecretKind},
};
use chaos_domain::merchant::StoreId;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, ApiJson, ApiPath, ApiResponse, ApiState, StoreContext};

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/stores/{store_id}/provider-secrets",
            post(create_provider_secret),
        )
        .layer(DefaultBodyLimit::max(20 * 1024))
}

#[derive(Deserialize)]
struct StorePath {
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
    StoreContext(actor): StoreContext,
    ApiPath(path): ApiPath<StorePath>,
    ApiJson(body): ApiJson<CreateProviderSecretBody>,
) -> Result<(HeaderMap, ApiResponse<ProviderSecretCreatedData>), ApiError> {
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
            actor: AdminActor::Store(actor),
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
