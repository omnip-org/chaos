//! Payment provider configuration, payment attempts, webhooks, and payment outbox work.
//!
//! The implementation is split by business responsibility while keeping one repository
//! module namespace. This makes the payment workflow discoverable without changing the
//! repository wiring used by the application layer.

include!("repository_core.rs");
include!("provider_accounts.rs");
include!("webhook_configuration.rs");
include!("payment_commands.rs");
include!("events.rs");
