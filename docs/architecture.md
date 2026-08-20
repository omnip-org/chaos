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

Commerce administration is exposed through MCP rather than an Admin HTTP API. HTTP remains for identity bootstrap, storefront and channel traffic, provider webhooks, health, and metrics.

## Code structure

Chaos remains a modular monolith with inward dependencies:

```text
chaos-api -------------> chaos-application -> chaos-domain
    |                            ^
    +-> chaos-infrastructure ----+

chaos-mcp -------------> chaos-application -> chaos-domain

chaos-worker ----------> chaos-application
       +---------------> chaos-infrastructure
```

- `chaos-domain` contains business types and rules without HTTP, SQL, cache, or serialization dependencies.
- `chaos-application` contains use cases and ports.
- `chaos-infrastructure` implements database, JWT, OIDC, cache, storage, and Provider adapters.
- `chaos-api` owns HTTP DTOs, extractors, routing, and dependency composition.
- `chaos-mcp` exposes commerce tools authenticated by User-owned Access Keys.
- the `chaos-worker` binary runs durable background consumers independently of API replicas.

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

## Request authorization

Every Store-owned table includes `store_id`. Every Store transaction sets transaction-local `app.store_id`; User directory reads additionally set `app.user_id`. PostgreSQL RLS is defense in depth, and the runtime role neither owns tables nor bypasses RLS.

Credential resolution is intentionally asymmetric:

- a User JWT yields `user_id`, followed by a Store membership check;
- a User Access Key yields `access_key_id` and `user_id`, followed by a fresh Store membership check;
- a Publishable Store Key yields `store_id`, optional `sales_channel_id`, and storefront capability scopes;
- a webhook yields `store_id` only after signature verification and Provider mapping;
- a Worker carries `store_id` in its durable job and establishes a fresh transaction context.

The MCP operation chain is `request_id -> access_key_id -> user_id -> store_id -> use case`. An MCP credential never contains a cached membership or role.

## Commerce reliability

PostgreSQL is the source of truth for catalogs, inventory, orders, payments, refunds, fulfillment, idempotency records, and durable jobs. Redis is limited to rate limiting and disposable coordination; losing Redis must not violate commerce invariants.

Money uses integer minor units plus an ISO currency. Orders snapshot the product, price, tax, discount, address, and Provider evidence required to preserve history. External Provider calls occur outside database transactions. Inbox and outbox records make webhook and Worker processing retryable and idempotent.

API replicas never start polling loops. `chaos-worker` is deployed and scaled independently. Every queue claim must remain safe with multiple Worker replicas; deployment may begin with one replica for cost, but correctness must not depend on singleton execution. Adaptive polling backoff limits idle database work, while leases, idempotency, retries, and bounded shutdown provide crash recovery.

## Incremental refactoring rule

Refactoring proceeds one bounded context at a time. A slice is complete only when its old code, configuration, schema objects, API contract, and stale documentation are removed and the required repository checks pass. Transitional naming in untouched contexts does not define the target model; this document does.
