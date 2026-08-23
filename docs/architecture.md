# Architecture

## Product boundary

Chaos is a commerce engine operated by Users and Stores:

```text
User ── Store Membership ── Store ── Sales Channel
  │                              │
  │                              ├── Catalog and variants
  │                              ├── Channel publication
  │                              ├── Orders and fulfillment
  │                              ├── Payments and refunds
  │                              └── Publishable channel keys
  │
  └── User-owned Access Key ── MCP tools ── Store membership authorization
```

A User may create and leave Stores, while Store Owners explicitly add Users and manage their roles. A Store is the tenant, authorization boundary, and commerce-data isolation boundary. There is no merchant-account layer. A Sales Channel controls where Store products are published; it is not an ownership boundary.

Human Users authenticate with an external identity provider and receive a short-lived Chaos JWT. Google and Apple are the initial providers behind one application port. The provider subject, not the email address, is the durable external identity.

Users create private Access Keys in the Identity control plane. An Access Key identifies one User, not one Store, and is never embedded in a storefront. It is a general trusted-client credential; MCP is its first consumer. Every Store-scoped request selects a Store explicitly and rechecks the User's current Store membership before invoking a commerce use case. Leaving a Store removes access immediately without rotating the User's Key.

The MCP HTTP transport is stateless. Protocol context and authentication are supplied on each request, allowing any API replica to handle it without local session affinity.

Stores issue public Storefront Keys for Sales Channels. These keys carry storefront capabilities and may select a Sales Channel; they cannot invoke trusted-client or administration use cases.

Commerce administration is exposed through MCP rather than an Admin HTTP API. HTTP remains for identity bootstrap, storefront and channel traffic, provider webhooks, and health checks.

## Code structure

Chaos remains a modular monolith with inward dependencies:

```text
chaos-api -------------> chaos-application -> chaos-domain
    |                            ^
    +-> chaos-infrastructure ----+

chaos-mcp -------------> chaos-application -> chaos-domain

chaos-api (Worker) ----> chaos-application
       +---------------> chaos-infrastructure
```

- `chaos-domain` contains business types and rules without HTTP, SQL, cache, or serialization dependencies.
- `chaos-application` contains use cases and ports.
- `chaos-infrastructure` implements database, JWT, OIDC, cache, storage, and Provider adapters.
- `chaos-api` owns HTTP DTOs, extractors, routing, and dependency composition.
- `chaos-mcp` exposes commerce tools authenticated by User-owned Access Keys.
- the `chaos-worker` binary runs durable background consumers independently of API replicas.

External adapters live under `chaos-infrastructure::integrations`,
`repositories`, `security`, and `storage`. The API and
Worker compose separate runtime dependency sets; starting a Worker does not
construct HTTP routes, MCP state, OIDC verification, or JWT services.

Bounded contexts may depend on another context only through an explicit application port. HTTP and MCP handlers do not execute SQL. Infrastructure records do not become domain entities.

## Identity

Identity owns:

- Users;
- external Provider identities;
- external identity verification;
- Chaos access-token issuance and verification;
- User-owned Access Key issuance, verification, listing, and revocation.

The database stores no passwords, magic links, passkeys, or human sessions. JWTs contain issuer, audience, subject, issued-at, and expiry claims and are signed with HS256. Provider ID tokens are accepted only after signature, algorithm, issuer, audience, expiry, subject, and verified-email validation against cached Provider JWKS.

Identity uses a dedicated non-owner database role because sign-in and Access Key authentication occur before any Store context exists. That role can access only the `identity` schema.

Automatic account linking by email is not supported. A different Provider presenting an email already assigned to a User receives a conflict. Explicit Provider linking can be added later as an authenticated use case.

Identity does not send authentication email. A verified Provider email is profile and
account-conflict data only; Google or Apple remains responsible for authenticating the
address.

## Request authorization

Every Store-owned table includes `store_id`. Every Store transaction sets transaction-local `app.store_id`; User directory reads additionally set `app.user_id`. PostgreSQL RLS is defense in depth, and the runtime role neither owns tables nor bypasses RLS.

Credential resolution is intentionally asymmetric:

- a User JWT yields `user_id`, followed by a Store membership check;
- a User Access Key yields `access_key_id` and `user_id`, followed by a fresh Store membership check;
- a Publishable Store Key yields `store_id` and a resolved `sales_channel_id`;
- a webhook yields `store_id` only after signature verification and Provider mapping;
- a Worker carries `store_id` in its durable job and establishes a fresh transaction context.

The MCP operation chain is `request_id -> access_key_id -> user_id -> store_id -> use case`. An MCP credential never contains a cached membership or role.

## Commerce reliability

PostgreSQL is the source of truth for catalogs, inventory, orders, payments, refunds, fulfillment, idempotency records, and durable jobs. Redis is limited to rate limiting and disposable coordination; losing Redis must not violate commerce invariants.

Money uses integer minor units plus an ISO currency. Orders snapshot the product, price, address, and Provider evidence required to preserve history. Stripe owns checkout tax and promotion calculation; its verified webhook writes subtotal, discount, tax, shipping, and total as Order facts. External Provider calls occur outside database transactions. Inbox and outbox records make webhook and Worker processing retryable and idempotent.

Orders use an internal UUID for joins and idempotency, plus a random shopper-facing
`W-YYYYMMDD-XXXXXXXX` order number for receipts, support, and MCP lookup. Guest order
tracking uses a Chaos-hosted URL with a fragment capability. The browser exchanges the
one-time-looking long-lived capability for a short-lived, store-bound session; only
digests are stored after a successful confirmation email delivery.

The Storefront identity is a Store-scoped persisted `commerce.shoppers` row. A
website visit creates one Shopper through `/store/v1/shopper-sessions`, and the
API returns a signed possession token for that row. The Shopper does not own a
Sales Channel and does not hold contact information; channel is request context,
while contact and address data are captured directly on the business Order.
Carts, Orders, Payments, and Analytics events carry the same `shopper_id`; there
is no Customer entity or visitor-to-Customer association table. An Order-bearing
Shopper is the buyer for all commerce and analytics purposes.

Store-owned data, including Stripe payment state, remains in the `commerce`
schema so Store-scoped foreign keys and RLS stay simple. Payment credentials,
readiness, payment queues, and verified webhook ingestion are logical modules,
not separate PostgreSQL schemas. The `commerce` migration is organized into logical modules: Store foundation,
Catalog, Pricing, Inventory, Search read model, Sales, Fulfillment configuration,
and Fulfillment execution. Generic idempotency, event routing, and analytics
delivery remain in the `integration` schema.

Cart and Order have separate responsibilities. The Checkout API transaction
creates a pending Order and reserves tracked inventory while leaving the Cart
active, then calls Stripe after the transaction commits and returns the
Embedded Checkout client secret in the same request. Stripe owns the checkout
UI, address, shipping, tax, and payment collection; Chaos stores the resulting
provider snapshot on the Order after a verified webhook. A successful payment
confirms the Order and completes the Cart; expiry or failure cancels the Order
and releases the reservation. There is no local Checkout aggregate to expire
or reconcile.

Analytics uses one append-only, Store-scoped behavior event ledger. The common
envelope contains `store_id`, `shopper_id`, `event_id`, `event_name`, and time;
event-specific values such as product, cart, order, session, traffic, and money
are stored in bounded `properties` JSON. `event_name` is validated only as a
lowercase snake-case identifier, not as a database enum, so new behaviors do
not require a migration. Provider delivery is an optional retryable projection
of stored events through destination and delivery records. See ADR 0026.

The `commerce` schema owns `payment_provider_accounts`, `provider_webhooks`,
payment readiness routines, and payment queue routines.
The Integration schema keeps one concise name for each generic responsibility:
`idempotency_keys`, `event_consumers`, `event_outbox`, `analytics_events`,
`analytics_destinations`, and `analytics_deliveries`. The last three form one chain: an internal Analytics
event is scheduled for a configured destination, then its delivery observation
is recorded by `destination_id` and `analytics_event_id`. Business outbox
routing is data-driven: `event_consumers.queue_name` points directly to the
PGMQ queue, while worker code owns only the payload semantics.

API replicas never start polling loops. `chaos-worker` is deployed and scaled independently. PGMQ owns durable message visibility, retry attempts, and concurrent claims; compact integration records retain the business payload and delivery outcome. Deployment may begin with one Worker replica for cost, but correctness does not depend on singleton execution. Adaptive polling backoff limits idle database work, while visibility timeouts, idempotent handlers, bounded retries, and bounded shutdown provide crash recovery. Scheduled reconciliation derived from authoritative rows continues to use short database leases because it is not an event queue. See ADR 0029.

## Incremental refactoring rule

Refactoring proceeds one bounded context at a time. A slice is complete only when its old code, configuration, schema objects, API contract, and stale documentation are removed and the required repository checks pass. Transitional naming in untouched contexts does not define the target model; this document does.
