# System Architecture

## 1. Architecture style

The first production version uses a modular monolith. The initial deployment units are a stateless API process and asynchronous worker processes that share the same domain code. Code is organized by business domain rather than by one global technical layer. Modules interact only through public application services or domain events.

This design keeps reliable transaction boundaries around checkout, inventory reservation, payment state, and refunds. Services should be extracted only when throughput, team ownership, or fault-isolation requirements justify the operational cost. Transactional outbox events provide future extraction seams.

```text
Storefront / Admin / Integrations
              |
         HTTP / Webhooks
              |
       Axum API (stateless)
              |
  +-----------+-----------+
  | catalog pricing cart  |
  | checkout order stock  |  <- domain modules
  | payment customer auth |
  +-----------+-----------+
              |
     PostgreSQL 18 (source of truth)
              |
       transactional outbox
              |
         Worker processes
              |
   Redis 8 (cache, rate limit, short locks)
```

Redis is never the source of truth for orders, inventory, or payments. Losing Redis data must not compromise business correctness.

## 2. Multi-account commerce model

The hierarchy is `user -> merchant_account -> store -> sales_channel`:

- A user is a global login identity and can own or join multiple merchant accounts.
- A merchant account is an isolated business workspace and the boundary for billing, membership, authorization, and RLS.
- A store is an independent online storefront within a merchant account and owns its domains and commerce configuration.
- A sales channel will define publication scope, API keys, and inventory-location selection for Web, mobile, POS, or marketplace clients.

Account and store resolution must never trust client-supplied identifiers by themselves:

- Admin API requests derive the merchant account from the authenticated user and membership.
- Storefront API requests derive merchant account, store, and channel from a publishable key or verified domain.
- Webhooks derive merchant account and store from a locally stored provider mapping after signature verification.
- Internal jobs carry `merchant_account_id` and `store_id` when applicable and establish a fresh account context in every consumer.

Every merchant-owned table contains `merchant_account_id`; store-owned commerce data also contains `store_id`. Relationships use account-scoped composite foreign keys to prevent cross-account references. Every account transaction sets `SET LOCAL app.merchant_account_id = ...`. PostgreSQL RLS provides defense in depth. The production application role must not own tables or have `BYPASSRLS`. Platform administration uses a separate role, connection pool, and audited execution path.

## 3. Money and multiple currencies

- Store money as `bigint amount_minor` plus `char(3) currency`. Never use floating-point types.
- Use uppercase ISO 4217 currency codes. Application-owned minor-unit metadata is versioned.
- Price lists store explicit amounts for each supported currency. Display conversion never overwrites a settlement price.
- Order creation snapshots product names, taxes, discounts, unit prices, currencies, and exchange rates. Historical orders do not change with the catalog.
- One order uses exactly one settlement currency. Payments and refunds must match the order currency.
- Exchange rates use fixed-point decimal values and record provider, timestamp, and the exact rate snapshot used.

The pricing domain will provide a Money value object with checked arithmetic, same-currency validation, explicit rounding, and deterministic remainder allocation.

## 4. Bounded contexts and dependency rules

Suggested implementation order:

1. identity: users, email links, passkeys, service accounts, and sessions;
2. merchant: merchant accounts, memberships, roles, API keys, stores, channels, and domains;
3. catalog: products, variants, options, collections, and media;
4. pricing: money, price lists, prices, promotions, and tax classes;
5. inventory: locations, stock items, reservations, and adjustments;
6. cart: carts, line items, and addresses;
7. checkout/order: idempotent order creation, state machines, and fulfillment state;
8. payment: provider accounts, payment intents, captures, refunds, and webhook inboxes;
9. customer: customers, addresses, and segments;
10. fulfillment: shipments, returns, and exchanges.

The Cargo workspace enforces dependency direction with separate packages:

```text
chaos-api -------------> chaos-application -> chaos-domain
    |                            ^
    +-> chaos-infrastructure ----+
```

- `chaos-domain` contains entities, value objects, aggregates, and pure business rules. It has no web, database, cache, or serialization dependencies.
- `chaos-application` contains use cases, transaction orchestration, and ports. It depends only on the domain package.
- `chaos-infrastructure` contains SQLx, Redis, and provider adapters that implement application ports.
- `chaos-api` contains Axum transport code, DTOs, authentication middleware, and the composition root.

Each bounded context keeps corresponding modules in the domain and application packages. Handlers must not contain SQL, and persistence records must not double as domain entities.

## 5. Consistency and reliability

- Every write API accepts `Idempotency-Key`, uniquely stored by `(merchant_account_id, key, operation)` with a request fingerprint and response snapshot.
- Inventory reservation uses PostgreSQL conditional updates or row locks with expiration. Redis may accelerate access but cannot own the invariant.
- Business changes and outbox events commit in the same PostgreSQL transaction.
- Workers claim outbox records with `FOR UPDATE SKIP LOCKED`. Delivery is at least once, so consumers must be idempotent.
- Provider webhooks are signature-verified and written to an inbox before asynchronous processing. Provider event IDs enforce deduplication.
- Payment providers are adapters. Payment state advances only from verified provider responses or webhooks.

## 6. API conventions

- Routes are grouped under `/admin/v1`, `/store/v1`, and `/webhooks/v1`. Health endpoints are under `/health`.
- IDs use UUIDv7. Time is stored as UTC `timestamptz` and emitted as RFC 3339.
- Pagination uses opaque cursors rather than large offsets.
- Successful responses use `{ "data": ..., "meta?": ... }`.
- Errors use `{ "error": { "code", "message", "details?" } }`.
- Every request generates or propagates `x-request-id`. Logs are structured tracing events.
- OpenAPI is the HTTP contract and will drive SDK generation or compatibility checks.

## 7. Security baseline

- Human accounts are passwordless. One-time email links provide initial sign-in and recovery, while WebAuthn passkeys provide phishing-resistant daily authentication.
- Users may register one or more passkeys. A second passkey is recommended but not required because verified email remains a recovery path.
- Raw email-link and session tokens are shown or delivered only to the client. PostgreSQL stores SHA-256 digests, expiration, and revocation state.
- WebAuthn registration and authentication state is stored only in Redis with a short TTL and atomic one-time consumption so ceremonies work across API instances without becoming replayable.
- Authentication abuse limits use privacy-preserving subject digests in Redis, so limits are shared by all instances without placing email addresses in cache keys.
- API keys use a searchable prefix plus a secret hash. Plaintext is shown exactly once.
- Admin authorization uses fine-grained RBAC. Sensitive writes enter an immutable audit log.
- Secrets come only from the runtime environment or a secret manager and never enter the repository or logs.
- Login, checkout, webhook, and public-key traffic use separate rate limits.
- CORS uses an allowlist. Request bodies have default size limits. External URL fetching must defend against SSRF.

## 8. PostgreSQL and Redis responsibilities

PostgreSQL stores all business entities, transactions, idempotency records, outbox and inbox records, and audit logs. Merchant-owned table indexes generally start with `merchant_account_id`. Large tables are partitioned only after query and scale evidence justifies it.

PostgreSQL schemas follow bounded-context ownership. Current and reserved schemas include `identity`, `merchant`, `catalog`, `pricing`, `inventory`, `sales`, `payments`, `fulfillment`, `integration`, `audit`, and `extensions`. Business SQL uses qualified identifiers. Detailed rules are defined in `docs/database-conventions.md`.

Redis provides short-lived cache entries, distributed rate limiting, session assistance, and short task coordination. Keys include environment and merchant account, for example `chaos:prod:ma:{merchant_account_id}:store:{store_id}:cart:{id}`, and cached values have TTLs. Lua or atomic commands protect compound cache invariants.

## 9. Deployment topology

The API is stateless and horizontally replicated. Workers scale by task category. Production should use managed PostgreSQL with point-in-time recovery, connection pooling, and appropriate replicas, plus highly available Redis. Migrations run as a separate release step and follow expand/migrate/contract. Application startup never runs migrations automatically.

Docker Compose runs blue and green API instances behind Caddy. A deployment replaces one instance at a time and waits for readiness before replacing the other. On SIGTERM, an instance starts draining and returns 503 from readiness, waits for Caddy to remove it, closes its listener, and lets Axum finish in-flight connections. Compose `stop_grace_period` provides the hard deadline. Configuration lives in `compose.ha.yaml` and `deploy/compose/Caddyfile`.

Application instances never own sessions, WebAuthn ceremonies, carts, or job ownership in local memory. Schedulers and workers use database claiming, leases, or leader election to prevent duplicate work across instances. Database migrations remain backward compatible for at least one release window so old and new versions can coexist briefly.

Observability targets include JSON logs, OpenTelemetry traces, and Prometheus metrics. Core metrics cover request latency and error rate, database pool pressure, Redis health, checkout conversion, payment failures, inventory reservation conflicts, and outbox lag.

## 10. Delivery roadmap

- Phase 0: workspace, local dependencies, configuration, health checks, logging, DDD boundaries, and the foundational merchant schema.
- Phase 1: identity, merchant-account membership, store use cases, transaction-scoped account context, RLS integration tests, and admin authentication.
- Phase 2: catalog, Money, price lists, and Storefront query APIs.
- Phase 3: inventory, carts, checkout, order state machines, and the idempotency framework.
- Phase 4: payment adapters, webhook inbox and outbox processing, and refunds.
- Phase 5: fulfillment, returns, search, production observability, and capacity testing.

Every phase requires migration tests, domain unit tests, cross-account isolation tests, HTTP integration tests, and an OpenAPI update.
