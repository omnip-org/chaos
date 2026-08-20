# ADR 0026: Use a Commerce Event Ledger and Provider Observations

- Status: Accepted
- Date: 2026-08-20

## Context

Chaos needs to preserve a Storefront conversion path, send the relevant events
to Meta reliably, and later compare commerce truth with Provider observations
such as Meta advertising spend and Stripe fees. The existing Analytics model
also sessionizes events, computes attribution, materializes daily reports, and
supports generic export destinations. Those responsibilities form a small CDP
and BI engine before the product needs either one.

The source of truth must remain understandable from one event stream. Derived
reports can be introduced when a concrete query requires them.

## Decision

Analytics owns three small models:

1. `commerce_events` is an append-only, Store-scoped ledger for browser and
   authoritative server events.
2. `meta_connections` and `meta_event_deliveries` configure Meta and deliver
   eligible events asynchronously through the independent Worker.
3. `provider_metric_snapshots` stores dated observations imported from external
   Providers without treating them as commerce truth.

The initial event vocabulary is:

- `page_view`;
- `view_content`;
- `search`;
- `add_to_cart`;
- `initiate_checkout`;
- `add_payment_info`;
- `purchase`;
- `refund`;
- `view_duration`.

The Meta adapter maps compatible events to Meta standard event names and sends
`view_duration` as a custom event. Browser and authoritative server events are
delivered through the Conversions API with their stable ledger `event_id`.
`purchase` is emitted only from authoritative payment capture, never from a
browser success page.

The Storefront SDK may additionally project consented events to Meta Pixel and
GA4. Meta Pixel uses the same event name and stable ledger `event_id` as CAPI;
the Pixel ID must match the configured Meta Dataset. GA4 disables automatic
PageView collection and receives Chaos-owned semantic events. These browser
Providers are optional projections and never determine ledger acceptance.

Every event carries `store_id`, `sales_channel_id`, source, occurrence and
receipt times, and bounded properties. Browser events additionally carry a
persistent `visitor_id`, a browser `session_id`, and the applicable consent.
Commerce references such as Product, Variant, Cart, Checkout, Order, Customer,
and Payment Attempt IDs use typed nullable columns when applicable. Money uses
integer minor units and an ISO currency.

Browser events may also carry one bounded traffic snapshot containing the
first touch, current browser-session touch, and latest non-direct touch. A
touch stores UTM source, medium, campaign, campaign ID, term, and content,
Referrer host, and consent-gated Meta or Google click IDs. It never stores the
full Referrer URL. These are immutable source facts, not computed attribution.
Authoritative payment and refund events inherit the latest snapshot associated
with their linked Visitor so the conversion path remains queryable.

Purchase is authoritative only after the payment Provider confirms capture.
The server ledger and Meta CAPI use the Order ID as the stable Purchase event
ID. After observing the confirmed Order, the Storefront SDK projects the same
ID and server-returned amount, currency, and lines to Meta Pixel and GA4. It
does not append a second browser Purchase to the ledger, and local browser
deduplication prevents success-page refreshes from projecting it twice.

Purchase is authoritative only after the payment Provider confirms capture.
The server ledger and Meta CAPI use the Order ID as the stable Purchase event
ID. After observing the confirmed Order, the Storefront SDK projects the same
ID and server-returned amount, currency, and lines to Meta Pixel and GA4. It
does not append a second browser Purchase to the ledger, and local browser
deduplication prevents success-page refreshes from projecting it twice.
AddPaymentInfo follows the same projection pattern using the Payment Attempt
ID after the server creates the attempt.

The Storefront SDK persists its bounded unsent queue in session storage,
retries with stable event IDs, drains all batches, and discards events before
the server's acceptance window expires. Engagement uses a monotonic clock,
pauses while the page is hidden or unfocused, and resumes after browser
back-forward cache restoration. Browser termination can still prevent final
delivery; authoritative commerce events never depend on that delivery.

`visitor_customer_links` associates a Storefront visitor with a Customer after
an authenticated or possession-bound customer interaction. It allows earlier
anonymous events and later commerce events to be queried as one path without
rewriting historical events or maintaining a derived Session aggregate.

Each Store chooses `opt_in` or `opt_out` for browser collection. Events record
`consent` or `store_policy` as their collection
basis, and the server verifies Store policy rather than trusting a public
client assertion. `opt_out` is the default and starts first-party collection and
configured Meta Pixel and GA4 projections immediately. A shopper opt-out stops
future browser collection and Provider projection. `opt_in` waits for explicit
storage consent. Identity linking follows the same collection basis when the
Store enables it, and stops after an explicit opt-out. Geographic policy
selection is not hard-coded into the SDK.

Store Analytics settings are one current Store-owned configuration record:
collection enabled, Meta reporting enabled, identity linking enabled, and raw
event retention days. Events retain the consent and setting revision that made
them eligible. The system keeps rate limiting, bounded retention, and deletion
by Visitor or Customer; it does not precompute sessions, attribution, or daily
reports.

Meta is the only outbound Analytics integration in this phase. Its connection
stores a Dataset ID, encrypted credential reference, optional test event code,
and enabled state. Delivery rows use leases, bounded retries, idempotency, and
record only bounded Provider responses. A failed Meta call never affects a
commerce transaction.

Provider metric snapshots are immutable observations keyed by Store, Provider,
external account, date, metric, dimensions, and currency. Examples include
Meta spend, impressions, clicks, and reported purchases, and Stripe gross,
fees, refunds, and net receipts. Import jobs and BI queries are deferred until
a concrete Provider integration or report is requested.

## Removed model

The following are not part of the target model:

- behavior-event processing and Session aggregates;
- commerce-fact duplication;
- attribution jobs and results;
- materialized daily Analytics reports;
- generic Analytics destinations and export deliveries;
- GA4 delivery.

## Consequences

- A Visitor path is a time-ordered query over one ledger rather than a join
  across raw events, Sessions, facts, and attribution results.
- Storefront and server events share one schema and idempotency rule.
- Meta delivery remains reliable but does not define Chaos event ownership.
- BI can compare authoritative commerce events with external observations
  without importing Provider semantics into Order or Payment aggregates.
- Aggregation remains a query concern until measured load or a concrete report
  justifies a projection.
