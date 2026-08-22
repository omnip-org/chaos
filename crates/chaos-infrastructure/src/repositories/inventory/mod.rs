//! Inventory balances, reservations, and append-only inventory transactions.
//!
//! The repository is split along the two inventory workflows: administrative balance
//! changes and shopper reservation transitions. Shared SQL helpers and snapshots stay
//! separate so an AI collaborator can locate the data path quickly.

include!("repository_core.rs");
include!("operations.rs");
include!("reservations.rs");
include!("snapshots.rs");
include!("persistence_helpers.rs");

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
