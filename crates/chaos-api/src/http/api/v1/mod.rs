//! Public channel API v1 endpoints grouped by capability.

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::Response,
};

use crate::http::ApiState;

mod attribution;
mod carts;
mod collections;
mod orders;
mod products;
mod shopper;
mod wire;

pub(crate) fn routes() -> Router<ApiState> {
    // Keep these public channel routes synchronized with the SDK
    // resources and wire types under packages/js/.
    Router::new()
        .merge(products::routes())
        .merge(collections::routes())
        .merge(shopper::routes())
        .merge(carts::routes())
        .merge(orders::routes())
        .layer(middleware::from_fn(storefront_cache_boundary))
}

async fn storefront_cache_boundary(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().append(
        header::VARY,
        HeaderValue::from_static("X-Chaos-Publishable-Key, X-Chaos-Shopper-Token"),
    );
    response
}
