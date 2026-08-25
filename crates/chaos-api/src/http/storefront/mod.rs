//! Public storefront HTTP endpoints grouped by capability.

use axum::Router;

use crate::http::ApiState;

pub(super) mod v1;

pub(crate) fn integration_routes() -> Router<ApiState> {
    v1::integration_routes()
}
