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

The ledger envelope is intentionally small: who (`shopper_id`), which browser
session when available (`session_id`), what (`event_name`), when
(`occurred_at`), a stable retry key (`event_id`), and event-specific
`properties`. The browser cannot declare `shopper_id` or change the Store
context. Normalized UTM values are stored in dedicated nullable columns;
traffic history, product IDs, order IDs, and money remain dynamic properties.

Browser events are appended by the Storefront analytics API. Cart, Checkout,
payment, and purchase events are appended directly by the repository
transaction that changes the corresponding commerce state. The generic
`integration.event_outbox` is reserved for asynchronous business workflows;
analytics does not add a second ingestion worker or translate outbox events
into another ledger.

`AnalyticsDeliveryWorker` is responsible only for scheduling, claiming,
retrying, and finishing external destination deliveries. Unknown event names
remain valid stored behavior and are handled by each provider adapter.

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
