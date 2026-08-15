# Delivery Roadmap

This document turns the architecture roadmap into verifiable delivery gates. A phase is complete only when every capability in its scope is implemented and every phase gate has current evidence. A schema placeholder or an accepted ADR is not an implemented capability.

## Status vocabulary

- `Delivered`: implemented and verified against every phase gate.
- `In progress`: usable foundations exist, but one or more capabilities or gates remain open.
- `Planned`: no end-to-end business capability has been delivered yet.

## Phase gates

Every phase requires:

1. domain unit tests for invariants and state transitions;
2. a clean bootstrap migration test on PostgreSQL 18;
3. PostgreSQL integration tests through the runtime role;
4. cross-account and cross-Store isolation tests for owned data;
5. HTTP integration tests covering success, authentication, authorization, validation, conflict, and not-found behavior;
6. a valid OpenAPI 3.1 contract whose local references resolve;
7. `cargo fmt`, workspace tests, and Clippy with warnings denied;
8. English-only code comments, documentation, and commit messages.

## Delivery status

| Phase | Status | Delivered foundation | Remaining outcome |
| --- | --- | --- | --- |
| 0 — Platform | Delivered | Rust workspace, Axum API, PostgreSQL 18, Redis 8, configuration, health endpoints, structured tracing, bounded Prometheus HTTP metrics, DDD crates, graceful shutdown, dual-instance Compose rollout, and a real-router PostgreSQL HTTP test harness | — |
| 1 — Identity and Merchant | Delivered | Passwordless email and passkey authentication, opaque sessions, merchant accounts, memberships, Store provisioning/configuration/lifecycle, Sales Channel administration, RLS context, directory queries, API key lifecycle, Admin OpenAPI, and full HTTP behavior matrices | — |
| 2 — Catalog and Pricing | Delivered | Product aggregate, Options, Variants, Sales Channels, transactional Product creation, Admin Product and Price List list/detail/update/lifecycle/publication with HTTP integration coverage, checked Money, multi-currency Variant prices, publishable Storefront Catalog list/detail queries, separate Admin and Store OpenAPI contracts, RLS, and all phase gates | — |
| 3 — Selling | Delivered | Shared idempotency records; location-aware stock balances; append-only inventory ledger; concurrency-safe, expiring reservation lifecycle; mutable Storefront Carts bound to one Store, Channel, currency, and Price List; atomic Checkout with immutable commercial snapshots and tracked-inventory reservations; immutable Order snapshots; explicit pending-to-confirmed/cancelled state machine; append-only transition audit trail; atomic reservation consumption/release; Admin and Store API contracts; clean PostgreSQL 18 bootstrap; runtime-role isolation, oversell, concurrency, immutability, and complete HTTP behavior gates | — |
| 4 — Payments | Delivered | Provider-neutral application port and sandbox adapter; currency-safe Payment Attempt and Refund state machines; immutable provider references; HMAC verification before tenant resolution; deduplicated durable webhook inbox; transactional outbox; multi-instance `SKIP LOCKED` workers; capped exponential retry and dead-letter handling; atomic capture-to-Order/inventory reconciliation; Store, Admin, and Webhook contracts; clean PostgreSQL 18 bootstrap; runtime-role isolation, cross-Store, concurrency, duplicate, out-of-order, and HTTP behavior gates | — |
| 5 — Operations | Delivered | Partial Fulfillment and shipment tracking; cancellation boundaries and reconciliation events; authorized Returns with receipt disposition, inventory restocking, refund coordination events, and immutable ledger evidence; transactional search events, multi-instance indexing workers, rebuildable duplicate-tolerant Store-isolated search; optional OTLP trace export and correlated structured worker logs; HTTP, business, dependency, database-pool, and queue metrics; an SLO dashboard; reproducible capacity thresholds; failure, replay, rotation, and rollback runbooks; Admin and Store contracts; clean PostgreSQL 18 bootstrap and end-to-end HTTP/database evidence | — |

## Phase 2 acceptance criteria

- Admin users can create, read, update, activate, archive, publish, and unpublish Products within an authorized Store.
- Product publication requires an active Product and an active Sales Channel in the same Store.
- `Money` uses checked minor-unit arithmetic and rejects mixed-currency operations.
- Price Lists define currency and activation context independently from Products.
- Variant prices are unique within their Price List and Store boundary.
- Storefront authentication resolves merchant account, Store, Sales Channel, key mode, and scopes from a publishable API key.
- Storefront Catalog queries return only active Store, Channel, Product, Variant, publication, Price List, and Price records.
- No Storefront response exposes drafts, archived records, unpublished Products, secret credentials, cost data, or another Store's rows.
- Admin and Storefront contracts are versioned separately.

## Phase 3 acceptance criteria

- Inventory is location-aware and mutations use an append-only ledger plus current balances.
- Reservations are concurrency-safe, expire deterministically, and cannot oversell tracked stock.
- Carts resolve a single Store, Sales Channel, currency, and price context.
- Checkout freezes an immutable commercial calculation before payment begins.
- Order creation snapshots product, Variant, pricing, discount, tax, currency, and customer-facing text.
- Cart, checkout, and order mutations are idempotent and safe under concurrent retries.
- Order transitions reject invalid state changes and record an audit trail.

## Phase 4 acceptance criteria

- Payment providers implement an application port and provider-specific data does not leak into the sales domain.
- Payment attempts and refunds have explicit state machines and immutable provider references.
- Webhooks are authenticated before tenant resolution and deduplicated in a durable inbox.
- Business transactions write outbox events atomically; workers claim, retry, and dead-letter jobs safely across instances.
- Duplicate, delayed, and out-of-order provider events cannot duplicate orders, captures, or refunds.

## Phase 5 acceptance criteria

- Fulfillment supports partial quantities, shipment tracking, cancellation boundaries, and order-state reconciliation.
- Returns support authorization, receipt, disposition, restocking decisions, and refund coordination.
- Search indexing is event-driven, rebuildable, Store-isolated, and tolerant of duplicate events.
- Traces, metrics, and structured logs correlate requests, jobs, merchant accounts, Stores, and external provider calls without leaking secrets.
- Capacity tests publish reproducible thresholds for API latency, checkout throughput, database pools, queues, and graceful rolling updates.
- Runbooks cover migration failure, dependency degradation, queue backlog, webhook replay, credential rotation, and rollback.

## Completion audit

The roadmap is complete only after a final clean-environment audit maps every criterion above to a test, command output, contract assertion, runtime probe, or operational artifact. Missing or indirect evidence keeps the corresponding phase open.

The final audit is recorded in `docs/completion-audit.md`.
