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
| 0 — Platform | In progress | Rust workspace, Axum API, PostgreSQL 18, Redis 8, configuration, health endpoints, tracing, DDD crates, graceful shutdown, and dual-instance Compose rollout | Roadmap-wide HTTP test harness and production telemetry gates |
| 1 — Identity and Merchant | In progress | Passwordless email and passkey authentication, opaque sessions, merchant accounts, memberships, Stores, RLS context, directory queries, API key lifecycle, and OpenAPI | Complete Admin HTTP integration coverage and remaining Store/channel administration |
| 2 — Catalog and Pricing | In progress | Product aggregate, Options, Variants, Sales Channels, transactional Product creation, Admin list/detail queries, checked Money, transactional Price List creation with multi-currency Variant prices, and RLS | Product lifecycle and publication, complete Price List administration, publishable Storefront Catalog API, and all phase gates |
| 3 — Selling | Planned | Shared idempotency record foundation | Inventory, carts, checkout, order state machines, reservations, snapshots, and all phase gates |
| 4 — Payments | Planned | Integration schema and PostgreSQL queue extensions | Payment provider ports, attempts, verified webhooks, inbox/outbox workers, refunds, and all phase gates |
| 5 — Operations | Planned | Structured logs, readiness/draining, and rolling-update smoke tooling | Fulfillment, returns, search, OpenTelemetry, metrics, SLO-oriented dashboards, capacity tests, and all phase gates |

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
