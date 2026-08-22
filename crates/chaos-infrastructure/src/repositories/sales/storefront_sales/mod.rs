//! Storefront sales persistence organized by the shopper-to-order workflow.
//!
//! The files share one module namespace to keep Repository implementations and private
//! domain reconstruction helpers compatible while making each workflow easy to locate.

include!("repository_core.rs");
include!("sales_commands.rs");
include!("cart.rs");
include!("checkout.rs");
include!("order.rs");
include!("snapshots.rs");
include!("sales_helpers.rs");

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
