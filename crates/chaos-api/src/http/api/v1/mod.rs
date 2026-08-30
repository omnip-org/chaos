//! Public channel API v1 endpoints grouped by capability.

use axum::Router;

use crate::http::ApiState;

mod analytics;
mod carts;
mod checkouts;
mod collections;
mod order_tracking;
mod products;
mod shopper;

pub(crate) fn routes() -> Router<ApiState> {
    // Keep these public channel routes synchronized with the SDK
    // resources and wire types under packages/js/.
    Router::new()
        .merge(products::routes())
        .merge(collections::routes())
        .merge(shopper::routes())
        .merge(carts::routes())
        .merge(checkouts::routes())
        .merge(order_tracking::routes())
        .merge(analytics::routes())
}
