# ADR 0034: Guest Order Lookup by Order Number and Email

- Status: Accepted
- Date: 2026-09-03

## Context

ADR 0027 gave guests a long-lived `ot_` tracking capability: minted at Order
confirmation, stored as a SHA-256 digest in `commerce.order_tracking_tokens`,
carried in the confirmation email's URL fragment, submitted to
`POST /api/v1/orders/tracking`, and swept by a Worker cleanup function after
180 days.

The capability adds a table, a token generator, an outbox payload secret, and a
retention job for one read path. A guest who still has the confirmation email
already knows the two things that identify their Order — the printed
`W-YYYYMMDD-XXXXXXXX` number and the email they gave at checkout — so the
storefront can offer a single lookup form instead of depending on an unbroken
capability link.

## Decision

`POST /api/v1/orders/lookup` accepts `{ order_number, email }`. The handler
authenticates with the Publishable Key like every storefront route, then matches
the number and the `commerce.orders.contact_email` (`citext`) within that key's
Store and Sales Channel. A match returns the same restricted projection the
tracking view used: identifiers, status, shipping locality and country, amount
breakdown, refunded amount, fulfillment shipping progress with carrier tracking,
and line items — no contact details and no full postal address, because
`(order number, email)` is a weaker credential than a random link.

Every miss — malformed input, unknown number, correct number with a
non-matching email, or an Order on another Channel — returns the same
`not_found`, so the endpoint cannot confirm which Order numbers exist.

The confirmation email links to the Sales Channel storefront origin's
`/orders/lookup` page with `order_number` and `email` pre-filled as query
parameters (ADR 0032 still owns the origin). No capability is minted or stored.

The `ot_` capability, `commerce.order_tracking_tokens`, its
`cleanup_expired_order_tracking_tokens` routine, the `order.confirmed` outbox
payload's `tracking_token`, and `POST /api/v1/orders/tracking` are removed.
`migrations/0008_drop_order_tracking.sql` drops the table and routine; the
tracking-free application must be deployed before that migration runs.

Per-request IP rate limiting is handled at the gateway and is out of scope for
this record. This decision deliberately does not add order-number lockout or
other distributed-enumeration defenses; the uniform `not_found` and the
restricted projection are the only application-level protections.

## Consequences

- One read path, no retention job, no secret in the outbox.
- A guest who loses the email can still self-serve from the number and email.
- `(order number, email)` is lower entropy than the old link; the response stays
  minimal and the gateway rate-limits the route.
- Storefronts that embedded the old `/orders/track#token=...` link must move to
  the `/orders/lookup` form. `orders.getTrackedOrder` in the JS SDK becomes
  `orders.lookupOrder({ orderNumber, email })`.
