# ADR 0030: Storefront Commerce Event Workflows

- Status: Accepted
- Date: 2026-08-22

## Decision

The Storefront conversion path has one consumer identity and one event ledger:

```text
signed shopper token
        ↓
Storefront API mutation / browser observation
        ↓
integration.commerce_events
        ↓
Analytics delivery task
        ↓
Meta CAPI (or a future destination)
```

`shopper_id` is issued and verified by Chaos. The SDK must send the signed
`x-chaos-shopper-token` when it flushes browser events. The request body never
contains a client-generated identity. `session_id` is retained only to group
events from one browser session.

The event ledger is deliberately small and append-only. It records who,
where, when, what happened, the bounded event properties, and the consent
snapshot. Product browsing is a browser observation. Cart, checkout, payment,
purchase, and refund outcomes are authoritative server events whenever the
backend has the state transition.

The event recorder and destination delivery are separate application services:

- `AnalyticsEventRecorder` consumes durable server outbox events and accepts
  browser observations into the ledger.
- `AnalyticsDeliveryWorker` schedules eligible ledger rows into
  `analytics_event_deliveries`, then calls the Provider adapter with bounded
  retries and stable event IDs.

`integration.outbox_events` remains because it is a durable workflow input for
cross-context commerce work. It is not an audit log. `webhook_inbox` and
idempotency records remain for Provider and command correctness.

The current audit surface is intentionally limited. Order and fulfillment
transition tables remain because they are queried by business flows or used as
idempotency evidence. Unread localization, media, collection, and review event
ledgers are removed. A future generic audit requirement should use one
Store-scoped `integration.audit_events` table with actor/subject identifiers and
bounded JSON metadata, without foreign-key joins to business aggregates. That
table is not created until a real reader exists.

## Consequences

- All funnel events can be queried by `shopper_id` without a visitor/customer
  link table.
- Meta delivery failure does not block recording or commerce mutations.
- Adding a destination creates Provider task behavior rather than another event
  ledger or another audit table.
- Old browser payloads containing `visitor_id` are intentionally unsupported;
  the contract is clean and the SDK queue key is versioned.
