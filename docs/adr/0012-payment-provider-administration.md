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
- one provider API credential reference;
- one webhook verification secret reference.

There is no separate enabled flag or provider health/readiness state. An account
is available for checkout when its required credentials are configured. Live
provider health verification and onboarding-specific state are deferred until a
provider needs them; they are not part of the account aggregate or checkout
routing decision.

The references identify values through the infrastructure secret resolver. They are not plaintext credentials and never appear in response DTOs, logs, events, or persistence snapshots. The current resolver stores AES-256-GCM-encrypted values as opaque `enc://` references in PostgreSQL; only the deployment encryption key remains outside the database. Responses expose only `credentials_configured`.

Storefront checkout accepts a typed provider choice, currently `stripe`, but
never accepts an account UUID from the browser. The backend resolves the one
matching account by `(store_id, provider)` and writes its immutable account UUID
to the Order when the Order is created. Payment attempts, refunds, commands, and
webhooks retain or resolve that same account UUID; workers never choose a
fallback account. Webhook tenant resolution uses the account UUID from the
endpoint path and the configured webhook secret.

Updating a Provider account replaces its two secret references atomically. A
changed outbound credential and webhook secret become active immediately; the
previous references are not retained and there is no verification overlap or
rollback deadline. Operators must coordinate provider-side changes with the
account update so in-flight requests and callbacks use the current references.

An account without the required credential is unavailable for new checkout
Orders. Existing Orders keep their Provider account relationship even if the
configuration is later changed. Secret replacement takes effect immediately, so
callbacks signed with a retired webhook secret are no longer accepted.

## Consequences

- Provider configuration is explicit, Store-isolated, RLS-protected, idempotent, and auditable through its creator and timestamps.
- Checkout cannot dispatch to an unconfigured provider.
- Historical Provider identity cannot drift through administrative updates.
- API and webhook secret values are never persisted in plaintext; only an opaque reference is stored, which may itself be an AES-256-GCM-encrypted value kept in PostgreSQL rather than an external secret manager.
- Credential rotation is explicit and immediate; deployments must coordinate replacement credentials and webhook endpoints.
- Provider health and onboarding checks can be added later without changing the
  Order's account binding model.
- A production adapter must resolve references through a dedicated infrastructure port and must never place resolved values in application or domain types.
