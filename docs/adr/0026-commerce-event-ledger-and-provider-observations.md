# ADR 0026: Use a Simple Behavior Event Ledger

- Status: Accepted
- Date: 2026-08-22

## Context

Chaos needs to collect Storefront behavior for analysis and optionally send
the same events to external destinations such as Meta CAPI. Analytics is not a
business audit log, privacy-policy engine, customer identity system, or BI
warehouse. Those responsibilities made the previous model harder to change
than the product requires.

## Decision

`integration.analytics_events` is the source of truth for behavior data. It is
an append-only, Store-scoped event ledger with this small common envelope:

- `store_id`;
- `shopper_id`;
- stable `event_id`;
- `event_name`;
- `occurred_at`, `received_at`, and `created_at`;
- bounded object `properties` JSON.

`event_name` is plain text with a lowercase snake-case format. The database
does not maintain an enum or event-specific constraints. A new behavior such
as `wishlist_added` can be stored without a migration. Product, cart,
Checkout, Order, Payment, session, traffic, money, and provider-specific
values belong in `properties`.

The signed Storefront Shopper token supplies `shopper_id`; the browser cannot
choose another shopper. Browser and server events use the same table and the
same stable event ID deduplication rule. Browser events are accepted directly
by the API. Commerce workflows append authoritative `add_to_cart`,
`initiate_checkout`, `add_payment_info`, `purchase`, and `refund` events in the
same transaction as their business state change. Analytics does not consume
the generic business `event_outbox`.

External delivery is a separate projection:

```text
analytics_events -> analytics_deliveries -> provider adapter
```

`analytics_destinations` contains provider configuration and enabled state.
`analytics_deliveries` contains retry status and provider references. A
destination failure never prevents event storage or a commerce transaction.
The Meta adapter maps known names to Meta standard events and passes unknown
names as custom events. It derives optional URL and money values from
`properties` instead of requiring fixed ledger columns.

The ledger remains partitioned daily by `received_at` using `pg_partman`.
`pg_cron` maintains future partitions; retention is manual. No analytics
policy table, consent snapshot, erasure workflow, metric snapshot, session
aggregate, attribution job, or automatic deletion exists in this model.

The SDK always uses the common envelope, stores session and traffic context in
`properties`, queues bounded batches, and retries with stable event IDs. It
also exposes `track(eventName, properties)` for Store-defined behavior names.
Browser provider scripts are optional projections and do not determine whether
an event is accepted by the Chaos ledger.

## Consequences

- Event collection is easy to extend and query.
- Analytics storage is independent from provider availability.
- The commerce model remains strictly typed; dynamic JSON is limited to
  behavior-specific analysis fields.
- Provider delivery and internal event storage can be inspected separately.
- Aggregation, attribution, retention, and privacy workflows can be added only
  when a concrete product requirement justifies them.
