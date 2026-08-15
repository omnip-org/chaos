# ADR 0009: Bind Storefront Resources to Shopper Credentials

- Status: Accepted
- Date: 2026-08-15

## Context

A publishable Store API key identifies a Store and Sales Channel, not an individual browser, guest, or Customer. It is intentionally present in public clients. Scoping a Cart, Checkout, Payment Attempt, or Order only by that key and an unguessable resource identifier does not establish shopper authorization and lets idempotency keys collide across unrelated shoppers in the same merchant account.

Phase 6 requires a possession-bound guest boundary before Storefront sales resources can be exposed to untrusted browser clients. Phase 7 may later associate that guest boundary with an authenticated Customer without changing ownership of resources that already exist.

## Decision

Chaos issues a signed, opaque-to-clients shopper credential through `POST /store/v1/shopper-sessions`. Cart creation requires that credential and echoes it alongside the Cart so clients can persist the complete Cart access state from either response. Cart, Checkout, Payment Attempt, and Order routes require both:

- the publishable API key in `Authorization`, which resolves merchant account, Store, Sales Channel, mode, and capability scopes;
- the shopper credential in `x-chaos-shopper-token`, which proves possession of the resource lineage.

The credential contains a version, signing-key identifier, random shopper identifier, Store identifier, and Sales Channel identifier authenticated with HMAC. Store and Channel binding prevents a token from crossing Storefront contexts and keeps shopper-scoped idempotency records isolated. It contains no email, Customer ID, permissions, or reusable provider credential. The verifier accepts the active signing key and explicitly configured overlapping verification keys so signing-key rotation does not invalidate active Checkouts during a rolling deployment.

The shopper-session endpoint generates the random shopper identifier before Cart persistence. The Cart stores it and returns it only through internal application DTOs. The API derives the signed credential from the persisted identifier, so an idempotent Cart-creation replay can reproduce the same credential without storing plaintext token material in PostgreSQL or an idempotency snapshot.

### Resource inheritance

Ownership is copied and constrained through the resource lineage:

```text
shopper_id
   |
 Cart
   |
 Checkout
   |
 Order
   |
 Payment Attempt
```

Composite foreign keys ensure a Checkout inherits the Cart's shopper identifier, an Order inherits the Checkout's identifier, and a Payment Attempt inherits the Order's identifier. Repository queries require merchant account, Store, Sales Channel, resource identifier, and shopper identifier where applicable. A credential mismatch returns the same not-found response as an unknown resource.

Admin users do not need a shopper credential. They continue to access authorized Store resources through merchant membership and role checks. Provider webhooks use verified provider-account mappings and provider object references, not shopper credentials.

### Idempotency

Storefront mutations use a `shopper` idempotency scope whose scope identifier is the shopper identifier. The Store and Sales Channel remain part of request authorization and request fingerprints. Two shoppers may safely choose the same textual `Idempotency-Key`; one shopper cannot replay or conflict with another shopper's response.

Shopper-session creation is intentionally stateless and creates no idempotency record. Once issued, the signed credential supplies the shopper scope for Cart creation and every descendant mutation. A Cart replay restores the persisted shopper identifier from the response snapshot and reproduces the same signed credential. Admin and pre-merchant-account operations retain their existing merchant-account and user scopes.

### Lifecycle and customer association

The shopper credential is a possession credential, not a login session. It authorizes only resources that carry its identifier and only while the accompanying publishable key still authorizes the route. Terminal resource state continues to reject invalid mutations independently of credential validity.

Phase 7 may link a shopper identifier to a Store-owned Customer after verified authentication or Checkout identification. That link is additive: it enables Customer order history and credential recovery but does not rewrite resource ownership or expose one Store's identity graph to another Store.

Compromise is handled by resource abandonment or a future explicit credential-rotation workflow. Operational signing-key rotation uses overlap keys. Tokens never appear in URLs, logs, metrics, traces, analytics events, or idempotency snapshots.

## Consequences

- A public Store key is no longer sufficient to access another shopper's sales resources.
- Guest clients must persist and send one additional header through the resource lifecycle.
- Storefront idempotency becomes correctly isolated between browsers and Customers.
- Signed derivation permits safe Cart-creation replay without recoverable token storage.
- Signing-key configuration and overlap rotation become production requirements.
- Customer authentication remains a separate Phase 7 capability instead of being conflated with guest possession.

## Rejected alternatives

### Treat UUIDv7 resource identifiers as authorization secrets

Identifier entropy reduces guessing but does not establish ownership, support revocation, or isolate idempotency. Identifiers belong in paths and logs; credentials do not.

### Store a plaintext Cart secret

Plaintext storage increases disclosure impact and complicates idempotent creation replay. A signed credential can be reproduced from the persisted shopper identifier without storing recoverable token material.

### Reuse the publishable API key as the shopper identity

The key is shared by every client of a Store or Channel. It identifies the Storefront application, not one shopper.

### Bootstrap Cart idempotency with the publishable key

A caller who learns another browser's idempotency key could replay Cart creation and receive its shopper credential. Idempotency keys are not possession credentials, so Chaos creates the shopper session before the first idempotent resource mutation.

### Require Customer login for every Cart

Mandatory login prevents normal guest commerce and couples Phase 6 authorization to the future Customer bounded context. Possession-bound guest credentials work before and after optional Customer association.
