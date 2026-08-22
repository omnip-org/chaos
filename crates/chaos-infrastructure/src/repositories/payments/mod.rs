//! Payment provider configuration, payment attempts, webhooks, and payment outbox work.
//!
//! The implementation is split by business responsibility while keeping one repository
//! module namespace. This makes the payment workflow discoverable without changing the
//! repository wiring used by the application layer.

include!("base.rs");
include!("provider_accounts.rs");
include!("readiness.rs");
include!("repository.rs");
include!("events.rs");
include!("snapshots.rs");
include!("shared.rs");
