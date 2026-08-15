# ADR 0012: Store-owned Payment Provider Administration

- Status: Accepted
- Date: 2026-08-16

## Context

Payment Attempts already reference `payments.provider_accounts`, but those rows were fixture-only and had no administrative lifecycle. Accepting a provider name from Storefront checkout without an explicitly administered Store mapping leaves provider selection ambiguous, makes credential handling ad hoc, and prevents a production Stripe adapter from resolving Store-specific configuration safely.

## Decision

A Payment Provider account is a Store-owned aggregate. A Store may configure at most one account for a canonical provider name. The provider name and external account reference become immutable after creation because Payment Attempts and verified webhooks depend on that identity mapping.

Owners and administrators may create and update Provider accounts through the Admin API. Other merchant roles may read the non-sensitive configuration but cannot change it. Administration supports:

- a bounded display name;
- enabled or disabled lifecycle state;
- one provider API credential reference;
- one webhook verification secret reference.

The references identify values in a deployment secret manager. They are not credentials, and raw credentials are never accepted or persisted by this API. References are write-only in OpenAPI and never appear in response DTOs, logs, events, or idempotency snapshots. Responses expose only `credentials_configured`.

Storefront Payment Attempt creation continues to accept a provider choice, but succeeds only when the current Store has exactly one enabled matching Provider account. Payment Attempts retain the Provider account foreign key. Webhook tenant resolution uses the immutable provider and external account mapping. Provider, Store, and merchant lifecycle changes do not suppress authenticated financial callbacks for existing activity.

Updating a Provider account replaces its two secret references atomically and may enable or disable new payment creation. Disabling blocks only new Payment Attempts. Existing Payment Attempts retain dispatch, client-action, refund, and signed-webhook access so in-flight money movement can converge. Existing Payment Attempts keep their Provider account relationship. Provider-specific onboarding state and dual-reference overlap rotation belong to later provider-integration increments.

## Consequences

- Provider configuration is explicit, Store-isolated, RLS-protected, idempotent, and auditable through its creator and timestamps.
- Checkout cannot dispatch to an unconfigured or disabled provider.
- Historical Provider identity cannot drift through administrative updates.
- API and webhook secrets remain outside PostgreSQL; only opaque references are stored.
- A production adapter must resolve references through a dedicated infrastructure port and must never place resolved values in application or domain types.
