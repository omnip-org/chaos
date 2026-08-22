//! Inventory domain model: locations, balances, and shopper reservations.

use std::collections::HashSet;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DomainError, FieldViolation, store::StoreId};

include!("ids.rs");
include!("locations.rs");
include!("balance.rs");
include!("reservations.rs");
include!("shared.rs");

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
