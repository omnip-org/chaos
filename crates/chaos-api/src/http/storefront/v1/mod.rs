//! Storefront API v1 endpoints grouped by public resource.

use axum::Router;

use crate::http::ApiState;

mod analytics;
mod carts;
mod collections;
mod orders;
mod products;
mod shopper_sessions;
mod webhooks;

pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .merge(products::routes())
        .merge(collections::routes())
        .merge(shopper_sessions::routes())
        .merge(carts::routes())
        .merge(orders::routes())
        .merge(analytics::routes())
        .merge(webhooks::routes())
}
