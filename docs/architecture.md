# System Architecture

Cross-cutting time handling follows [Time conventions](time-conventions.md).
External payment, shipping, and notification integrations follow [ADR 0007](adr/0007-external-provider-boundaries.md).
The first carrier integration follows [ADR 0015](adr/0015-easypost-shipping-adapter.md).
First-party behavior analytics and conversion exports follow [ADR 0008](adr/0008-first-party-analytics-and-conversion-exports.md).
Storefront sales resources follow [ADR 0009](adr/0009-possession-bound-shopper-credentials.md).
Fulfillment and Return reconciliation follows [ADR 0014](adr/0014-fulfillment-and-return-reconciliation.md).

## 1. Architecture style

The first production version uses a modular monolith. The current binary hosts the stateless HTTP server plus Checkout expiry, payment, fulfillment, and search worker loops; database claims preserve multi-instance correctness. Workers recover abandoned leases, stop claiming when the instance begins draining, and receive a bounded interval to finish in-flight batches before forced cancellation. Worker categories may later become independent deployment units. Code is organized by business domain rather than by one global technical layer. Modules interact only through public application services or domain events.

This design keeps reliable transaction boundaries around checkout, inventory reservation, payment state, and refunds. Services should be extracted only when throughput, team ownership, or fault-isolation requirements justify the operational cost. Transactional outbox events provide future extraction seams.

```text
Storefront / Admin / MCP / Integrations
              |
         HTTP / Webhooks
              |
       Axum API (stateless)
              |
  +-----------+-----------+
  | catalog pricing cart  |
  | checkout order stock  |  <- domain modules
  | payment fulfillment   |
  +-----------+-----------+
              |
     PostgreSQL 18 (source of truth)
              |
       transactional outbox
              |
         Worker processes
              |
   Redis 8 (rate limits and short-lived auth state)
```

Redis is never the source of truth for orders, inventory, or payments. Losing Redis data must not compromise business correctness.

## 2. Multi-account commerce model

The hierarchy is `user -> merchant_account -> store -> sales_channel`:

- A user is a global login identity and can own or join multiple merchant accounts.
- A merchant account is an isolated business workspace and the boundary for billing, membership, authorization, and RLS.
- A store is an independent online storefront within a merchant account and owns its domains and commerce configuration.
- A sales channel defines publication scope and API keys for Web, mobile, POS, or marketplace clients. Inventory-location selection remains planned.

Account and store resolution must never trust client-supplied identifiers by themselves:

- Admin API requests derive the merchant account from the authenticated user and membership.
- Storefront API requests currently derive merchant account, Store, and Channel from a publishable key. Verified-domain resolution is reserved.
- Future MCP requests will derive merchant account and Store from a scoped secret key; each tool will also enforce its capability scope.
- Webhooks derive merchant account and store from a locally stored provider mapping after signature verification.
- Internal jobs carry `merchant_account_id` and `store_id` when applicable and establish a fresh account context in every consumer.

Every merchant-owned table contains `merchant_account_id`; store-owned commerce data also contains `store_id`. Relationships use account-scoped composite foreign keys to prevent cross-account references. Every account transaction sets `SET LOCAL app.merchant_account_id = ...`. PostgreSQL RLS provides defense in depth. The production application role must not own tables or have `BYPASSRLS`. Platform administration uses a separate role, connection pool, and audited execution path.

## 3. Money and multiple currencies

- Store money as `bigint amount_minor` plus `char(3) currency`. Never use floating-point types.
- Use uppercase ISO 4217 currency codes. Application-owned minor-unit metadata is versioned.
- Price lists store explicit amounts for each supported currency. Display conversion never overwrites a settlement price.
- Order creation snapshots product names, deterministic tax and discount allocations, Tax Rule evidence, unit prices, Price List tax semantics, shipping selection, and currency. Historical orders do not change with catalog or configuration updates.
- One order uses exactly one settlement currency. Payments and refunds must match the order currency.
- Exchange-rate display and settlement conversion are reserved capabilities. Authoritative Price Lists currently provide explicit prices in each enabled settlement currency.

The pricing domain provides a Money value object with checked arithmetic, same-currency validation, explicit rounding, and deterministic remainder allocation.

## 4. Bounded contexts and dependency rules

Suggested implementation order:

1. identity: users, email links, passkeys, and sessions; service accounts are reserved;
2. merchant: merchant accounts, memberships, roles, API keys, Stores, and Channels; domain resolution is reserved;
3. catalog: products, variants, options, and publication; collections and media are reserved;
4. pricing: Money, price lists, prices, Store Tax Rules, and Store Promotions;
5. inventory: locations, stock items, reservations, and adjustments;
6. sales: Store Customers, saved addresses, carts, line items, Checkout, Orders, and immutable contact/address snapshots;
7. fulfillment: allocations, shipments, Returns, and future shipping-provider coordination;
8. payment: provider accounts, Payment Attempts, captures, Refunds, and webhook inboxes;
9. customer: Customer profiles and saved addresses currently live within Sales because association is part of checkout ownership; segmentation remains a future boundary that may justify extraction;
10. notifications: semantic, versioned delivery requests; a recoverable email worker; Store-isolated suppression and delivery status; Resend production delivery and signed webhooks; and SMTP development delivery;
11. analytics: behavior events, consent evidence, immutable Store policies, consent-aware Customer identity links, data-subject erasure audit, sessions, versioned attribution, and future aggregates and conversion exports behind the same boundary.

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

- Every write API accepts `Idempotency-Key` with a request fingerprint and response snapshot. Records are uniquely scoped by `(scope, scope_id, operation, key)`: authenticated user scope is used before a merchant account exists, and merchant-account scope is used for merchant-owned operations.
- Inventory reservation uses PostgreSQL conditional updates and row locks. A recoverable scheduler leases due Checkouts and atomically closes the Checkout, releases active reservations, updates stock balances, and writes the inventory ledger. Redis may accelerate access but cannot own the invariant.
- Business changes and outbox events commit in the same PostgreSQL transaction.
- Every Outbox event type references an immutable consumer registry. A registered event has at most one owner; claim functions verify that owner before using `FOR UPDATE SKIP LOCKED`. Unowned events remain pending and visible in the consumer backlog until a consumer is deliberately assigned. Delivery is at least once, so consumers must be idempotent.
- Provider webhooks are signature-verified and written to an inbox before asynchronous processing. Provider event IDs enforce deduplication.
- Payment providers are adapters. Store-owned Provider accounts contain immutable external identity mappings and opaque secret-manager references; API responses never expose those references. Payment state advances only from verified provider responses or webhooks.
- Store-owned Shipping Services and destination regions are Fulfillment data. Sales requests a server-authoritative quote and freezes the selected service, amount, currency, and delivery estimate in Checkout and Order snapshots. Store-owned Shipping Provider Accounts hold provider identity, enablement, a default origin, and opaque credential references; secret material remains external. Provider rate requests, immutable Rates, and purchased Label evidence remain Fulfillment-owned. A recoverable tracking worker is the only external-observation path that may advance a shipped Fulfillment to delivered. External shipping providers remain Fulfillment infrastructure adapters. Notification providers deliver semantic requests without owning commerce state.
- Store-owned Promotions remain in Pricing. Checkout evaluates active automatic rules plus an optional redemption code, chooses one deterministic best discount, allocates it across merchandise lines before tax, and freezes the selected rule evidence into Checkout and Order snapshots.
- Analytics accepts untrusted browser behavior only through its bounded collection contract. Sales, Payments, Fulfillment, Shipping, and Returns append dedicated transactional Outbox events; the Analytics consumer re-reads their owning tables before creating typed immutable facts with the Outbox ID as the stable identity. Analytics never becomes the source of truth for commerce state or authorization.

## 6. API conventions

- Routes are grouped under `/admin/v1`, `/store/v1`, and `/webhooks/v1`. Health endpoints are under `/health`; the internal Prometheus scrape endpoint is `/metrics`.
- IDs use UUIDv7. Time is stored as UTC `timestamptz` and emitted as RFC 3339.
- Pagination uses opaque cursors rather than large offsets.
- Successful responses use `{ "data": ..., "meta?": ... }`.
- Errors use `{ "error": { "code", "message", "details?" } }`.
- Every request generates or propagates `x-request-id`. Logs are structured tracing events.
- OpenAPI is the HTTP contract. SDK generation and automated compatibility checks are planned.

## 7. Security baseline

- Human accounts are passwordless. One-time email links provide initial sign-in and recovery, while WebAuthn passkeys provide phishing-resistant daily authentication.
- Users may register one or more passkeys. A second passkey is recommended but not required because verified email remains a recovery path.
- Raw email-link and session tokens are shown or delivered only to the client. PostgreSQL stores SHA-256 digests, expiration, and revocation state.
- WebAuthn registration and authentication state is stored only in Redis with a short TTL and atomic one-time consumption so ceremonies work across API instances without becoming replayable.
- Authentication abuse limits use privacy-preserving subject digests in Redis, so limits are shared by all instances without placing email addresses in cache keys.
- API keys use a searchable prefix plus a secret hash. Plaintext is shown exactly once.
- Human sessions and Store API keys are not interchangeable authentication mechanisms. Future MCP credentials will remain a separate scope boundary.
- Admin authorization uses merchant roles. Fine-grained permissions and a general immutable audit log remain production-readiness requirements; existing domain ledgers and transition records cover only their owned workflows.
- Secrets come only from the runtime environment or a secret manager and never enter the repository or logs.
- Login abuse limits are implemented. Separate checkout, webhook, and public-key limits remain required before production exposure.
- CORS allowlists and explicit route-class request body limits remain required before browser production exposure. Future external URL fetching must defend against SSRF.

## 8. PostgreSQL and Redis responsibilities

PostgreSQL stores current business entities, transactions, idempotency records, outbox and inbox records, domain-specific ledgers, immutable initial browser evidence, and the initial rebuildable analytics session projection. A general audit log is planned. Merchant-owned table indexes generally start with `merchant_account_id`. High-volume analytical scans remain excluded from the OLTP pool; partitioning or an external analytics store is selected only after query and scale evidence justifies it.

PostgreSQL schemas follow bounded-context ownership. Current and reserved schemas include `identity`, `merchant`, `catalog`, `pricing`, `inventory`, `sales`, `payments`, `fulfillment`, `integration`, `audit`, and `extensions`. Business SQL uses qualified identifiers. Detailed rules are defined in `docs/database-conventions.md`.

Redis currently provides distributed authentication rate limiting and short-lived WebAuthn ceremony state. Future caches and short coordination keys must include environment and ownership context, use TTLs, and preserve PostgreSQL as the source of truth.

## 9. Deployment topology

The API is stateless and horizontally replicated. Checkout expiry, payment, and search workers currently run inside each API process and use database claims. Checkout expiry, payment inbox, and payment outbox claims recover processing rows after a one-minute lease, while shutdown stops new claims and waits up to `SHUTDOWN_WORKER_TIMEOUT_MS` for active batches. Later deployment units may scale worker categories independently. Production should use managed PostgreSQL with point-in-time recovery, connection pooling, and appropriate replicas, plus highly available Redis. Migrations run as a separate release step and follow expand/migrate/contract. Application startup never runs migrations automatically.

Docker Compose runs blue and green API instances behind Caddy. A deployment replaces one instance at a time and waits for readiness before replacing the other. On SIGTERM, an instance starts draining and returns 503 from readiness, waits for Caddy to remove it, closes its listener, and lets Axum finish in-flight connections. Compose `stop_grace_period` provides the hard deadline. Configuration lives in `compose.ha.yaml` and `deploy/compose/Caddyfile`.

Application instances never own sessions, WebAuthn ceremonies, carts, or job ownership in local memory. Schedulers and workers use database claiming, leases, or leader election to prevent duplicate work across instances. Analytics workers build session projections continuously and run bounded retention deletion once per minute; Store policy changes may shorten existing expiry but never extend it. Database migrations remain backward compatible for at least one release window so old and new versions can coexist briefly.

The platform emits structured logs, optional OpenTelemetry traces, and Prometheus metrics for bounded HTTP routes, database pool pressure, dependency health, checkout conversion, payment failures, inventory reservation conflicts, queue lag, and analytics sessionization health. The Compose gateway blocks public access to `/metrics`; collectors scrape instances on the internal service network.

## 10. Delivery roadmap

- Phase 0: workspace, local dependencies, configuration, health checks, logging, DDD boundaries, and the foundational merchant schema.
- Phase 1: identity, merchant-account membership, store use cases, transaction-scoped account context, RLS integration tests, and admin authentication.
- Phase 2: catalog, Money, price lists, and Storefront query APIs.
- Phase 3: inventory, carts, checkout, order state machines, and the idempotency framework.
- Phase 4: payment adapters, webhook inbox and outbox processing, and refunds.
- Phase 5: fulfillment, returns, search, production observability, and capacity testing.
- Phase 6: shopper ownership, scheduler and worker recovery, event consumers, and transaction hardening.
- Phase 7: customers, addresses, shipping, tax, promotions, and real checkout totals.
- Phase 8: Stripe, Resend, shipping adapters, provider administration, and reconciliation.
- Phase 9: first-party analytics, behavior collection, attribution, reporting, and conversion destinations.
- Phase 10: domains, richer Catalog, outbound integrations, MCP, SDKs, and ecosystem compatibility.

Every phase requires migration tests, domain unit tests, cross-account isolation tests, HTTP integration tests, and an OpenAPI update.
