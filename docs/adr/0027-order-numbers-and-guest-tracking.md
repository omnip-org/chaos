# ADR 0027: Separate Order Identity, Display, and Guest Tracking

- Status: Accepted; the guest-tracking capability is superseded by ADR 0034
- Date: 2026-08-20

## Decision

Orders retain UUIDv7 primary keys for internal relationships, API operations,
and Analytics deduplication. Every Order also receives one immutable,
Store-scoped display number in `W-YYYYMMDD-XXXXXXXX` format. The date is UTC
and the suffix uses cryptographically secure Crockford Base32 characters.
There is no sequential component. A unique database constraint rejects the
extremely unlikely collision.

Guest tracking no longer treats the Order UUID as a credential. **ADR 0034
supersedes the capability mechanism described in the rest of this section**: the
`ot_` capability, its `commerce.order_tracking_tokens` table, the outbox
payload's `tracking_token`, and `POST /api/v1/orders/tracking` are removed. A
guest now reads an Order with `POST /api/v1/orders/lookup`, supplying the
`W-YYYYMMDD-XXXXXXXX` number plus the contact email on the Order. The
order-number decision above is unchanged.

There is no public Order detail or Order-ID checkout recovery endpoint. An
active Publishable Key authorizes the Store API entry point; the order-number +
email match is the only public path for a guest to read an Order.

Carrier tracking URLs remain data shown inside the Chaos order view. Emails link
to Chaos rather than making a carrier URL the primary customer entry point.
