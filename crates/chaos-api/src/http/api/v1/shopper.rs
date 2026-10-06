//! Anonymous shopper session issuance and last-touch refresh.

use axum::http::HeaderMap;
use axum::{Router, extract::State, routing::post};
use chaos_core::sales::{ShopperSessionContext, UtmTags};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{ApiError, ApiQuery, ApiResponse, ApiState, PublishableChannel, ShopperContext};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/shopper/sessions", post(create_session))
        .route("/shopper/sessions/touch", post(touch_session))
}

/// Campaign tags the storefront appends from its own page URL, e.g.
/// `POST /shopper/sessions?utm_source=newsletter&utm_medium=email`.
#[derive(Deserialize)]
pub(super) struct UtmQuery {
    utm_source: Option<String>,
    utm_medium: Option<String>,
    utm_campaign: Option<String>,
    utm_term: Option<String>,
    utm_content: Option<String>,
}

#[derive(Serialize)]
struct ShopperSessionResponse {
    shopper_id: Uuid,
    shopper_token: String,
}

/// `X-Real-IP` is set by `deploy/nginx` from the real client address; the
/// browser cannot report its own IP. UTM comes from the query string —
/// the storefront copies it off its own page URL.
fn session_context(headers: &HeaderMap, utm: UtmQuery) -> ShopperSessionContext {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    ShopperSessionContext {
        user_agent: header("user-agent"),
        ip_address: header("x-real-ip"),
        utm: UtmTags {
            source: utm.utm_source,
            medium: utm.utm_medium,
            campaign: utm.utm_campaign,
            term: utm.utm_term,
            content: utm.utm_content,
        },
    }
}

// ===== POST /shopper/sessions =====

async fn create_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiQuery(utm): ApiQuery<UtmQuery>,
    PublishableChannel(actor): PublishableChannel,
) -> Result<ApiResponse<ShopperSessionResponse>, ApiError> {
    let shopper_id = state
        .storefront_sales
        .create_shopper(&actor, session_context(&headers, utm))
        .await?;
    let shopper_token = state.shopper_credentials.issue(&actor, shopper_id)?;
    Ok(ApiResponse::created(ShopperSessionResponse {
        shopper_id: shopper_id.as_uuid(),
        shopper_token: shopper_token.expose_secret().to_owned(),
    }))
}

// ===== POST /shopper/sessions/touch =====

/// Refreshes `shoppers.attribution.last_seen` with the caller's current
/// journey UTM, for a returning visitor who came back through a different
/// campaign. Requires a shopper token; `first_seen` is never touched, and
/// a request with no UTM is a server-side no-op. Returns `{ "data": null }`
/// with `200`.
async fn touch_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ApiQuery(utm): ApiQuery<UtmQuery>,
    ShopperContext(actor): ShopperContext,
) -> Result<ApiResponse<()>, ApiError> {
    state
        .storefront_sales
        .refresh_shopper_seen(&actor, session_context(&headers, utm))
        .await?;
    Ok(ApiResponse::ok(()))
}
