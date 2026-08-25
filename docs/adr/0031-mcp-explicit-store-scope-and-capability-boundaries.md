# ADR 0031: Use Explicit Store Scope in MCP Tool Inputs

- Status: Accepted
- Date: 2026-08-25

## Context

Chaos MCP operates multiple Stores with one User-owned Access Key. A custom
`X-Chaos-Store-Id` request header is transport-specific and is easy for an AI
client to omit or accidentally encode as a tool argument. The tool schema is
the interface the model actually sees, so Store scope must be represented in
that schema.

The MCP surface also contains several different boundaries: User and Store
administration, catalog and pricing, order operations, fulfillment, payment
provider administration, and analytics. Provider transport records and queue
leasing are Integration concerns, not ordinary Store mutations.

## Decision

Every Store-scoped MCP tool declares a required `store_id: string` input and
passes that value through the common Access Key and membership authorization
path. The server does not read Store scope from an HTTP header. The only
User-scoped exceptions are `create_store` and `list_stores`, because they run
before a target Store exists or intentionally enumerate the User's Stores.

Closed operational values exposed by MCP use JSON Schema enums, including Store
roles, order status filters, review status filters, and provider secret kinds.
Open-ended provider event names, analytics event names, and provider-specific
payload values remain strings or bounded JSON because adding a provider event
must not require an MCP or database migration.

All writes retain explicit `confirm: true` semantics. Manual order confirmation
uses the same `order.confirmed` outbox event as payment-driven confirmation, so
email and downstream consumers observe one business event regardless of the
originating operation.

## Boundaries and follow-up

- Integration account configuration is exposed only through capability-specific
  tools today: Stripe account administration, provider secret creation, Meta
  destination configuration, and the manual shipping account listing.
- Webhook ingress remains HTTP: verify the signature first, persist recognized
  and unknown provider events in the shared Integration inbox, then let the
  Worker classify an unknown event as `unsupported`.
- Queue claims, webhook replay, dead-letter handling, and provider-account
  health are operational Integration capabilities. They should receive a
  separate read-only/operations surface after core query contracts exist,
  rather than being mixed into catalog or order tools.
- Shipping remains manual until a real carrier contract and shipping-amount
  write model exist. MCP can create fulfillments and move fulfillment state;
  it does not invent a quote or mutate `orders.shipping_amount_minor`.
