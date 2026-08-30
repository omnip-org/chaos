# ADR 0030: Storefront Behavior Event Flow

- Status: Accepted
- Date: 2026-08-22

## Decision

The Storefront path uses one Shopper identity and one behavior event ledger:

```text
signed shopper token
        ↓
browser observation or commerce state change
        ↓
integration.analytics_events
        ↓
optional analytics_deliveries
        ↓
Meta CAPI or another destination
```

The ledger envelope is intentionally small: which Store and Channel
(`store_id`, `channel_id`), who (`shopper_id`), which browser session when
available (`session_id`), the normalized source (`event_source`), what
(`event_name`), when (`occurred_at`), a stable retry key (`event_id`), and
event-specific `properties`. The browser cannot declare `shopper_id`, change
the Store context, or bind an event to another Channel. Normalized UTM values
are stored in dedicated nullable columns;
traffic history, product IDs, order IDs, and money remain dynamic properties.
For commerce items, `product_id` and `product_variant_id` are canonical. The
Meta adapter derives its content ID from `product_variant_id` when present,
otherwise from `product_id`.

Generic browser observations and client commerce events are appended by the
Storefront analytics API. The public collection path rejects only `purchase`,
which remains payment-confirmation-owned. Before a cart or checkout mutation,
the browser SDK prepares one commerce envelope containing the event ID and
attribution, but sends the business request without an analytics field. After
the mutation or checkout session succeeds, the SDK sends the envelope to the
same `/analytics/events` endpoint, adds canonical response values, and projects
the same event ID to browser providers. Meta Pixel and the ledger-backed CAPI
delivery can therefore deduplicate. Cart and Checkout repositories do not
write analytics rows. The browser-side checkout event stores the captured
attribution and its canonical `order_id`; the later payment webhook looks up
that exact event instead of querying the latest browser event. No attribution
field is added to the Order. The generic
`integration.event_outbox` is reserved for asynchronous business workflows;
analytics does not add a second ingestion worker or translate outbox events
into another ledger.

`AnalyticsDeliveryWorker` is responsible only for scheduling, claiming,
retrying, and finishing external destination deliveries. Unknown event names
remain valid stored behavior and are handled by each provider adapter.

Destination enablement is forward-only. Creating or re-enabling a destination
records an activation watermark (including a UUIDv7 boundary when no event is
currently available), so retained events from before activation or during a
disabled interval are not replayed automatically. Any historical backfill must
be an explicit, bounded, and auditable operation.

The MCP surface exposes destination configuration and event querying. Event
queries return the dynamic properties because this is an internal behavior
analysis ledger, not an audit abstraction. There is no Analytics policy tool;
the destination `enabled` field is the only provider delivery switch.

## Consequences

- A website visit creates one persisted Shopper and all subsequent events can
  be queried by `shopper_id`.
- New behavior names do not require a database migration or Rust enum update.
- Provider failures do not block event collection or commerce transactions.
- The system does not persist consent, policy revisions, audit observations,
  metric snapshots, erasure requests, or automatic retention state.
