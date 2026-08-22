//! Fulfillment, shipping, returns, and their asynchronous workflow handlers.
//!
//! Files are organized by the business workflow rather than by database operation.
//! They intentionally share this module namespace so the repository API stays stable.

include!("base.rs");
include!("repository.rs");
include!("events.rs");
include!("returns.rs");
include!("queries.rs");
include!("snapshots.rs");
include!("shared.rs");
