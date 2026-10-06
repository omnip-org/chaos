//! Anonymous shopper session issuance and last-touch refresh.

use axum::{Router, extract::State, http::HeaderMap, routing::post};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::http::{
    ApiError, ApiJson, ApiResponse, ApiState, PrivateApiResponse, PublishableChannel,
    ShopperContext,
};

use super::attribution::{SessionAttributionRequest, shopper_session_context};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/shopper/sessions", post(create_session))
        .route("/shopper/sessions/touch", post(touch_session))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShopperSessionRequest {
    #[serde(default)]
    attribution: Option<SessionAttributionRequest>,
}

#[derive(Serialize)]
struct ShopperSessionResponse {
    shopper_id: Uuid,
    shopper_token: String,
}

// ===== POST /shopper/sessions =====

async fn create_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    PublishableChannel(actor): PublishableChannel,
    ApiJson(request): ApiJson<ShopperSessionRequest>,
) -> Result<PrivateApiResponse<ShopperSessionResponse>, ApiError> {
    let shopper_id = state
        .storefront_sales
        .create_shopper(
            &actor,
            shopper_session_context(request.attribution.as_ref(), &headers),
        )
        .await?;
    let shopper_token = state.shopper_credentials.issue(&actor, shopper_id)?;
    Ok(ApiResponse::created(ShopperSessionResponse {
        shopper_id: shopper_id.as_uuid(),
        shopper_token: shopper_token.expose_secret().to_owned(),
    })
    .private())
}

// ===== POST /shopper/sessions/touch =====

/// Refreshes `shoppers.attribution.last_seen` with the caller's current
/// journey UTM, for a returning visitor who came back through a different
/// campaign. Requires a shopper token; `first_seen` is never touched, and
/// an attribution body with no UTM is a server-side no-op. Returns
/// `{ "data": null }` with `200`.
async fn touch_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ShopperContext(actor): ShopperContext,
    ApiJson(request): ApiJson<ShopperSessionRequest>,
) -> Result<ApiResponse<()>, ApiError> {
    state
        .storefront_sales
        .refresh_shopper_seen(
            &actor,
            shopper_session_context(request.attribution.as_ref(), &headers),
        )
        .await?;
    Ok(ApiResponse::ok(()))
}

#[cfg(test)]
mod tests {
    use axum::http::header;
    use serde_json::json;

    use super::*;

    #[test]
    fn session_attribution_uses_the_same_nested_utm_body_as_checkout() {
        let request: ShopperSessionRequest = serde_json::from_value(json!({
            "attribution": {
                "utm": {
                    "source": "newsletter",
                    "campaign": "fall"
                }
            }
        }))
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "2001:db8::1".parse().unwrap());
        headers.insert(header::USER_AGENT, "test-browser".parse().unwrap());

        let context = shopper_session_context(request.attribution.as_ref(), &headers);

        assert_eq!(context.ip_address.as_deref(), Some("2001:db8::1"));
        assert_eq!(context.user_agent.as_deref(), Some("test-browser"));
        assert_eq!(context.utm.source.as_deref(), Some("newsletter"));
        assert_eq!(context.utm.campaign.as_deref(), Some("fall"));
    }

    #[test]
    fn session_attribution_rejects_the_old_flat_utm_shape() {
        assert!(
            serde_json::from_value::<ShopperSessionRequest>(json!({
                "utm_source": "newsletter"
            }))
            .is_err()
        );
    }
}
