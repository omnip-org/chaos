# ADR 0022: Unauthenticated Single-Order Status Lookup

- Status: Superseded by ADR 0027
- Date: 2026-08-17

## Context

ADR 0009 bound Cart, Checkout, Order, and Payment Attempt access to a signed, possession-bound shopper credential, and explicitly rejected treating a resource's UUIDv7 identifier as sufficient authorization on its own: *"Identifier entropy reduces guessing but does not establish ownership, support revocation, or isolate idempotency. Identifiers belong in paths and logs; credentials do not."* That reasoning is correct for every resource it was written against — carts and checkouts are mutable, and a leaked identifier without possession binding would let a third party keep modifying someone else's in-progress purchase.

A guest "check my order status" page is a different shape of problem: it is read-only, it is reached from a link a shopper received (typically by email) on a device that never held the original shopper token, and prior storefront integrations against other commerce backends have shipped this exact pattern successfully — the Order ID itself, being an unguessable UUID, is the whole credential, the same trust model a capability link already uses. Requiring the original shopper token for this one read would force a Storefront to either give up the "works from any device, any time" property or start persisting long-lived shopper tokens across sessions, which reintroduces exactly the credential-lifecycle problems ADR 0009 was written to avoid elsewhere.

## Decision

`GET /storefront/v1/orders/{order_id}` accepts a Publishable Key holding the `orders:read` scope with no shopper credential. A Store must explicitly mint a key with this scope for the capability to exist at all; it is not implicitly enabled for every Publishable Key.

This is a narrow, scoped carve-out, not a reversal of ADR 0009:

- Every other shopper-lineage resource (Cart, Checkout, Payment Attempt, and Order *mutation*) keeps its possession-bound requirement unchanged.
- The endpoint is read-only. There is no idempotency key to collide and no mutation to attribute to the wrong shopper — the two concerns ADR 0009's rejected-alternative reasoning was protecting.
- The lookup is scoped to one Order by its own UUIDv7 primary key within the credential's Store and Sales Channel; it cannot list, filter, or enumerate.

`StorefrontSales::get_order_by_id` (`crates/chaos-core/src/sales/mod.rs`) and the `OrderLookupMachine` extractor (`crates/chaos-api/src/http/shared/extract.rs`) implement this alongside the existing shopper-bound `get_order`, which remains available for a shopper still in possession of their token. Infrastructure reuses `load_order` unchanged — it never filtered by shopper in the first place — and simply skips the `ensure_order_owner` check that only the possession-bound path calls.

## Consequences

- A Store that wants "email a status link" behavior for guest orders can mint a Publishable key with `orders:read`; a Store that doesn't want this exposure at all simply never grants the scope.
- Anyone who obtains an Order ID (a leaked log line, a shared link, a browser history entry) can read that Order's contact, address, and line-item detail. This is the same exposure the capability-link pattern already accepts by design; the Storefront is expected to apply the same protections such a link already needs regardless of backend — no-referrer, `private, no-store`, and no-index on the page that renders it — since none of those are enforceable by Chaos itself.
- The response does not yet include a payment-status summary; a shopper checking whether payment succeeded still needs the separate, possession-bound Payment Attempt endpoint. Folding a coarse payment status into this response is a reasonable follow-up but is deliberately out of scope here to keep this change to the authorization boundary alone.

## Rejected alternatives

### Require the Storefront to persist and resend the original shopper token

Keeps the letter of ADR 0009 but pushes the cost onto the client: a long-lived, cross-session shopper token stored to support a status-check link is a *wider* possession-bound credential than the one ADR 0009 designed for cart/checkout mutation, not a narrower one.

### A separate email-plus-order-number lookup endpoint

Closer to some other backends' guest order lookup shape, but requires Chaos to authenticate an email address against Order contact data with no rate-limit-resistant credential of its own — a small, unguessable ID plus a coarse per-key scope is a smaller surface than an endpoint that has to defend against email enumeration.
