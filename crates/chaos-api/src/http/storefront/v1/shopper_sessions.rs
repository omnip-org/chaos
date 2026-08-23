use axum::{Router, extract::State, routing::post};
use secrecy::ExposeSecret;
use serde::Serialize;

use crate::http::{ApiResponse, ApiState, CartMachine};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new().route("/shopper-sessions", post(create_shopper_session))
}

#[derive(Serialize)]
struct ShopperSessionData {
    shopper_token: String,
}

async fn create_shopper_session(
    State(state): State<ApiState>,
    CartMachine(actor): CartMachine,
) -> Result<ApiResponse<ShopperSessionData>, crate::http::ApiError> {
    let shopper_id = state.storefront_sales.create_shopper(&actor).await?;
    let shopper_token = state.shopper_credentials.issue(&actor, shopper_id)?;
    Ok(ApiResponse::created(ShopperSessionData {
        shopper_token: shopper_token.expose_secret().to_owned(),
    }))
}
