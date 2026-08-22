//! Storefront sales persistence organized by the shopper-to-order workflow.
//!
//! The files share one module namespace to keep Repository implementations and private
//! domain reconstruction helpers compatible while making each workflow easy to locate.

include!("base.rs");
include!("repository.rs");
include!("cart.rs");
include!("checkout.rs");
include!("order.rs");
include!("snapshots.rs");
include!("shared.rs");

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
