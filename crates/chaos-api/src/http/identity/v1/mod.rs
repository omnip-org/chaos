//! Identity API v1 endpoints grouped by public resource.

use axum::Router;

use crate::http::ApiState;

mod access_keys;
mod auth;

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .merge(auth::routes())
        .merge(access_keys::routes())
}
