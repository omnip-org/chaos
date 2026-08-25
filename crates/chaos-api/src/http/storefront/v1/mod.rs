//! Storefront API v1 endpoints grouped by public resource.

use axum::Router;

use crate::http::ApiState;

mod analytics;
mod carts;
mod collections;
mod orders;
mod products;
mod shopper;
mod webhooks;

pub(crate) fn routes() -> Router<ApiState> {
    // Keep these public resource routes synchronized with the Storefront SDK
    // resources and wire types under packages/js/.
    Router::new()
        .merge(products::routes())
        .merge(collections::routes())
        .merge(shopper::routes())
        .merge(carts::routes())
        .merge(orders::routes())
        .merge(analytics::routes())
}

pub(crate) fn integration_routes() -> Router<ApiState> {
    webhooks::routes()
}
