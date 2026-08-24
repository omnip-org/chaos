# ADR 0012: Store-owned Payment Provider Administration

- Status: Amended by ADR 0024
- Date: 2026-08-16

## Context

Payment Attempts already reference Provider account records, but those rows were fixture-only and had no administrative lifecycle. Accepting a provider name from Storefront checkout without an explicitly administered Store mapping leaves provider selection ambiguous, makes credential handling ad hoc, and prevents a production Stripe adapter from resolving Store-specific configuration safely.

## Decision

A Payment Provider account is a Store-owned aggregate in the `integration`
schema. A Store may configure at most one account for a typed provider. The
provider and Chaos-generated account UUID become immutable after creation
because Orders and verified webhooks depend on that identity mapping. The
initial supported provider is `stripe`; Embedded Checkout is the initial Stripe
payment flow, not a separate provider type.

Owners administer Provider accounts through MCP tools, as established by ADR 0025. Other Store roles may read non-sensitive configuration according to the current membership policy but cannot change it. Administration supports:

- a bounded display name;
- enabled or disabled lifecycle state;
- one provider API credential reference;
- one webhook verification secret reference.

Creation and update explicitly request the enabled state. Disabled configuration can be stored before external onboarding is complete. Enabling invokes the configured provider's onboarding-readiness port before persistence. A ready assessment is stored with its check time and a bounded normalized provider snapshot. An unsuccessful assessment stores stable blocker codes and leaves the account disabled. Provider-specific response types and identity data never enter this aggregate or its public response.

The references identify values through the infrastructure secret resolver. They are not plaintext credentials and never appear in response DTOs, logs, events, or persistence snapshots. The current resolver stores AES-256-GCM-encrypted values as opaque `enc://` references in PostgreSQL; only the deployment encryption key remains outside the database. Responses expose only `credentials_configured`.

Storefront Payment Attempt creation continues to accept a provider choice, but succeeds only when the current Store has exactly one enabled matching Provider account. Payment Attempts retain the Provider account foreign key. Webhook tenant resolution uses the immutable provider and Chaos account UUID from the endpoint path, and currently requires the Provider account to remain enabled. Operators must keep an account enabled until its in-flight financial callbacks have converged.

Updating a Provider account replaces its two secret references atomically and may enable or disable new payment creation. A changed outbound credential and webhook secret become active immediately; the previous references are not retained and there is no verification overlap or rollback deadline. Operators must coordinate provider-side changes with the account update so in-flight requests and callbacks are signed with the current references.

Disabling blocks new Payment Attempts and webhook ingestion for that Provider account. Existing Payment Attempts keep their Provider account relationship, but outstanding callbacks must be drained before disabling. Secret replacement takes effect immediately, so callbacks signed with a retired webhook secret are no longer accepted. Provider-specific onboarding state belongs to later provider-integration increments.

## Consequences

- Provider configuration is explicit, Store-isolated, RLS-protected, idempotent, and auditable through its creator and timestamps.
- Checkout cannot dispatch to an unconfigured or disabled provider.
- Historical Provider identity cannot drift through administrative updates.
- API and webhook secret values are never persisted in plaintext; only an opaque reference is stored, which may itself be an AES-256-GCM-encrypted value kept in PostgreSQL rather than an external secret manager.
- Credential rotation is explicit and immediate; deployments must coordinate replacement credentials and webhook endpoints.
- Live traffic cannot be enabled from an unchecked or action-required provider assessment.
- A production adapter must resolve references through a dedicated infrastructure port and must never place resolved values in application or domain types.
