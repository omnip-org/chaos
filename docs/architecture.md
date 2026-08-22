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

Users create private Access Keys in the Identity control plane. An Access Key identifies one User, not one Store, and is never embedded in a storefront. Every MCP request selects a Store explicitly and rechecks the User's current Store membership before invoking a commerce use case. Leaving a Store removes access immediately without rotating the User's Key.

The MCP HTTP transport is stateless. Protocol context and authentication are supplied on each request, allowing any API replica to handle it without local session affinity.

Stores issue only Publishable Keys for Sales Channels. These keys carry storefront capabilities and may select a Sales Channel; they cannot invoke MCP or administration use cases.

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

External adapters live under `chaos-infrastructure::providers`. The API and
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

Money uses integer minor units plus an ISO currency. Orders snapshot the product, price, tax, discount, address, and Provider evidence required to preserve history. External Provider calls occur outside database transactions. Inbox and outbox records make webhook and Worker processing retryable and idempotent.

Orders use an internal UUID for joins and idempotency, plus a random shopper-facing
`W-YYYYMMDD-XXXXXXXX` order number for receipts, support, and MCP lookup. Guest order
tracking uses a Chaos-hosted URL with a fragment capability. The browser exchanges the
one-time-looking long-lived capability for a short-lived, store-bound session; only
digests are stored after a successful confirmation email delivery.

The Storefront identity is a Store-scoped persisted `commerce.shoppers` row. A
website visit creates one Shopper through `/store/v1/shopper-sessions`, and the
API returns a signed possession token for that row. The Shopper does not own a
Sales Channel and does not hold contact information; channel is request context,
while email, phone, and addresses remain immutable Checkout and Order snapshots.
Carts, Checkouts, Orders, Payments, and Analytics events carry the same
`shopper_id`; there is no Customer entity or visitor-to-Customer association
table. An Order-bearing Shopper is the buyer for all commerce and analytics
purposes.

The commerce database remains one physical `commerce` schema so Store-scoped
foreign keys and RLS stay simple, but its migration is organized into logical
modules: Store foundation, Catalog, Pricing, Inventory, Search read model,
Sales, Fulfillment configuration, Payments, and Fulfillment execution. Provider
calls and durable delivery state remain outside these business tables in the
Integration workflow.

Cart and Checkout have separate lifecycles. Creating a Checkout reserves
inventory and freezes a Checkout snapshot, but leaves the Cart active. A Cart
allows at most one pending Checkout at a time; expired Checkouts remain as
history and the Cart can start a new Checkout. Only successful Order creation
completes both the Checkout and the Cart. This allows payment or expiry retries
without rewriting an existing Checkout snapshot.

Analytics uses one append-only, Store-scoped Analytics Event ledger for the
Storefront conversion path and authoritative server events. External provider
delivery is a retryable projection of eligible events, with provider-neutral
destination and delivery records and provider-specific adapters. Provider metrics
are not persisted; Chaos does not
precompute Sessions, attribution, or daily reports without a concrete product
query. Browser events retain bounded first-touch,
browser-session, and last-non-direct traffic facts so UTM conversion paths can
be queried without introducing an attribution engine. A Store may authorize
browser collection through an `opt_in` or `opt_out` Store policy; every event
records whether explicit consent or Store policy was its basis. The default
`opt_out` policy starts configured Meta Pixel and GA4 projections immediately
and stops them after a shopper opt-out. Meta Pixel shares stable event IDs with
CAPI for deduplication. See ADR 0026.

The Integration schema keeps one concise name for each responsibility:
`idempotency_keys`, `provider_webhooks`, `event_consumers`, `event_outbox`,
`analytics_policy`, `analytics_events`, `analytics_destinations`, and
`analytics_deliveries`. The last three form one chain: an internal Analytics
event is scheduled for a configured destination, then its delivery observation
is recorded by `destination_id` and `analytics_event_id`.

API replicas never start polling loops. `chaos-worker` is deployed and scaled independently. PGMQ owns durable message visibility, retry attempts, and concurrent claims; compact integration records retain the business payload and delivery outcome. Deployment may begin with one Worker replica for cost, but correctness does not depend on singleton execution. Adaptive polling backoff limits idle database work, while visibility timeouts, idempotent handlers, bounded retries, and bounded shutdown provide crash recovery. Scheduled reconciliation derived from authoritative rows continues to use short database leases because it is not an event queue. See ADR 0029.

## Incremental refactoring rule

Refactoring proceeds one bounded context at a time. A slice is complete only when its old code, configuration, schema objects, API contract, and stale documentation are removed and the required repository checks pass. Transitional naming in untouched contexts does not define the target model; this document does.
