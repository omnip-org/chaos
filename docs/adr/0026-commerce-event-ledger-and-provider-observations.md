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
an append-only, Store- and Channel-scoped event ledger with this small common
envelope:

- `store_id`;
- `channel_id`;
- `shopper_id`;
- optional `session_id` (the browser session UUID);
- optional `utm_source`, `utm_medium`, `utm_campaign`, `utm_term`, and
  `utm_content` attribution values;
- stable `event_id`;
- `event_name`;
- normalized `event_source` (`browser` or `server`);
- `occurred_at`, `received_at`, and `created_at`;
- bounded object `properties` JSON.

`event_name` is plain text with a lowercase snake-case format. The database
does not maintain an enum or event-specific constraints. A new Store-defined
behavior name can be stored without a migration. Product, cart,
Checkout, Order, Payment, traffic history, money, and provider-specific values
belong in `properties`. Commerce item properties use `product_id` and
`product_variant_id`. Single-item events expose the same IDs at the event top
level, while multi-item events repeat them inside `items[]`. The Meta adapter
uses the variant ID as the Meta content ID when present, otherwise the product
ID. The five normalized UTM columns are populated from
explicit top-level `utm_*` values when present, otherwise from the current
browser session's `traffic.session` values; first-touch and last-non-direct
history remains in `properties`.

The signed Storefront Shopper token supplies `shopper_id`; the browser cannot
choose another shopper. Browser and server events use the same table and the
same stable event ID deduplication rule. Generic browser observations and
client commerce events are accepted through the same public analytics API. For
a commerce action, the SDK prepares one envelope with the event ID and
attribution before calling the cart or checkout API, while the business
request remains analytics-agnostic. Only after a successful response does the
SDK merge canonical response values, enqueue the event through
`/analytics/events`, and project the same event ID to browser providers. A
failed business operation produces no client commerce event; the SDK persists
and retries a successfully queued event independently of provider delivery.
Analytics does not consume the generic business `event_outbox`.

The browser-side `initiate_checkout` event stores the captured attribution in
`integration.analytics_events` together with the public `order_number` property.
When a payment webhook appends `purchase`, it looks up that exact checkout
event by Store, Channel, Shopper, source Cart, and order number, rather than
querying the shopper's latest browser event. This keeps `fbc`, `fbp`, session,
traffic, UTM, URL, and network context tied to the checkout that produced the
Order without adding attribution fields to `commerce.orders`. `purchase` is
never accepted from the generic browser collection path; payment confirmation
remains its only ledger source.

External delivery is a separate projection:

```text
analytics_events -> analytics_deliveries -> provider adapter
```

`analytics_destinations` contains provider configuration and enabled state.
`analytics_deliveries` contains retry status and provider references. A
destination failure never prevents event storage or a commerce transaction.
The Meta adapter projects only the supported event subset and marks other
stored behavior names as filtered deliveries. `page_view` remains in the
first-party ledger but is intentionally filtered from the server-side Meta
CAPI projection for now. The adapter derives optional URL and money values
from `properties` instead of requiring fixed ledger columns.

The ledger remains partitioned daily by `received_at` using `pg_partman`.
`pg_cron` maintains future partitions; Analytics event retention is manual. No
analytics policy table, consent snapshot, erasure workflow, metric snapshot,
session aggregate, attribution job, or automatic Analytics event deletion
exists in this model.

The SDK always uses the common envelope, sends the session UUID for the API to
normalize into the `session_id` column, stores traffic context in `properties`
for attribution history, queues bounded batches, and retries with stable event
IDs. It also exposes `track(eventName, properties)` for Store-defined behavior
names.

Server-side storefront bridges pass their inbound request context to the SDK's
request-scoped client. The SDK copies the edge-observed client IP into each
analytics event before forwarding it to Chaos, while the browser SDK remains
the source of the client user-agent and other browser metadata. The analytics
API preserves event-provided IP and user-agent values and only uses request
cookies as a fallback for missing `fbc` and `fbp`; it does not derive network
metadata from the forwarding request's IP or user-agent headers.

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
