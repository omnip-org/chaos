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
| 3 — Selling | Delivered | Shared idempotency records; possession-bound shopper sessions and Cart-to-Order ownership; location-aware stock balances; append-only inventory ledger; concurrency-safe reservation transitions; mutable Storefront Carts bound to one Store, Channel, currency, and Price List; atomic Checkout with immutable commercial snapshots and tracked-inventory reservations; immutable Order snapshots; explicit pending-to-confirmed/cancelled state machine; append-only transition audit trail; atomic reservation consumption/release; recoverable automatic Checkout and reservation expiry; Admin and Store API contracts; runtime-role isolation, oversell, concurrency, expiry recovery, and immutability tests | — |
| 4 — Payments | Delivered | Provider-neutral application ports; sandbox and production Stripe Connect direct-charge adapters; on-demand possession-bound client handoff without persisted client secrets; Store-owned Provider account administration with immutable identity mapping, write-only secret-manager references, enable/disable lifecycle, deterministic checkout resolution, RLS, and Admin contracts; stable outbound idempotency keys; Stripe account scoping and API-version pinning; timestamped signed webhooks; currency-safe Payment Attempt and Refund state machines; authenticated webhook inbox; transactional outbox; recoverable leases; bounded worker drain; retry/dead-letter handling; atomic capture reconciliation; real HTTP adapter tests and PostgreSQL 18 phase-gate evidence | — |
| 5 — Operations | In progress | Partial Fulfillment and shipment tracking; recoverable event-driven Order fulfillment and delivery reconciliation; authorized Returns with deterministic line-total allocation, receipt disposition, inventory restocking, and exactly-once refund coordination; transactional search events; rebuildable Store-isolated search; optional OTLP trace export; bounded business and queue metrics; current capacity harness and disposable seed; hardened non-root release containers; verified zero-failure rolling update; operations runbooks; Admin and Store contracts | Retain the full production-like 10-minute capacity report. |
| 6 — Transaction Hardening | Delivered | Existing idempotency, inbox, outbox, RLS, and state-machine foundations; possession-bound shopper credentials and constrained Cart-to-Payment ownership lineage; deterministic multi-instance Checkout and reservation expiry; stale scheduler, payment outbox, and webhook lease recovery; bounded graceful worker drain; abandoned-lease replay evidence; immutable event-consumer registry enforced by Outbox foreign keys; owner-checked claims; visible unreconciled backlog for unowned events | — |
| 7 — Real Checkout | Delivered | Guest and authenticated Customer checkout; Store-scoped Customer profiles and saved addresses; immutable shopper association and recoverable Customer Order history; admin Order list and status, Customer, and email filters; canonical contact and validated addresses; Store-owned Shipping Services, Tax Rules, and Promotions; deterministic discount/tax allocation; explicit recalculation rules; immutable Checkout-to-Order commercial snapshots; Store and Admin contracts and full phase-gate evidence | — |
| 8 — Provider Integrations | Delivered | Stripe Connect direct charges, responsibility-gated onboarding, PaymentIntent and Refund dispatch, client handoff, timestamped signed webhooks, bounded credential overlap, and periodic readiness reconciliation; Resend semantic delivery, durable leases, stable idempotency, signed raw-body webhook reconciliation, and Store-isolated suppression; capability-specific Provider and secret ports; Store-owned EasyPost administration and origin; durable normalized rate evidence; crash-safe label reconciliation and purchase; explicit cancellation evidence; and recoverable tracking reconciliation that alone advances delivered Fulfillments | — |
| 9 — Analytics and Attribution | Delivered | Versioned, consent- and policy-gated first-party browser collection; Store-scoped rate limiting and deduplication; bounded visible-and-focused engagement estimation; recoverable sessionization; retention, identity linking, and erasure; transactional canonical commerce facts; delayed and rebuildable first- and last-touch attribution; isolated daily read models; Store-owned Meta CAPI and GA4 destination administration with write-only environment-secret references; eligibility-gated, idempotent, recoverable export delivery; pseudonymous provider identities; and provider-specific infrastructure adapters using the Order ID for browser/server event deduplication | — |
| 10 — Extensibility and Ecosystem | In progress | Versioned Admin, Store, and Webhook contracts; scoped API keys; search read model; Store-owned custom Domains bound to Web Sales Channels; one-time digest-backed DNS TXT verification; immutable lifecycle events; exact-hostname public resolution; Store-owned manually ordered Collections with independent lifecycle and publication; verified direct-upload Product Media with an S3-compatible port, audit, RLS, and ready-only Storefront filtering; Store-scoped typed Catalog localization with deterministic Storefront fallback and immutable Cart-to-Order snapshots; and a typed `@chaos-commerce/js` client covering all 24 Store API operations plus bundled first-party analytics | Outbound webhooks; MCP; compatibility automation; third-party application workflows. |

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

## Phase 6 acceptance criteria

- A publishable Store key cannot read or mutate another shopper's Cart, Checkout, Payment Attempt, or Order without the resource's shopper credential.
- Reservation and Checkout expiry run automatically and remain correct across concurrent schedulers.
- Every event type has one declared consumer owner; unowned events remain visible and are never reported as reconciled.
- Processing leases are recovered after worker termination without duplicate business effects.
- Shutdown stops new claims and completes or safely releases in-flight work before process exit.
- Integration tests terminate workers between claim and completion, advance the Clock, replay events, and verify convergence.

## Phase 7 acceptance criteria

- Guest and authenticated-customer checkout capture validated contact, billing, and shipping snapshots without exposing customer data across Stores.
- Shipping options are quoted for shippable lines and the selected service, amount, and delivery estimate are frozen in the Order.
- Tax and promotion calculations allocate deterministic line and Order totals in one settlement currency.
- Checkout recalculation has explicit rules for address, shipping, inventory, pricing, tax, and promotion changes.
- Admin users can list and filter Orders while shoppers can access only their own Order history or possession-bound guest Orders.

## Phase 8 acceptance criteria

- Payment, shipping, and notification providers implement capability-specific application ports; provider SDK types remain in infrastructure.
- Stripe onboarding records the selected merchant-of-record, funds-flow, fee, liability, dispute, and payout model before live payments are enabled.
- Provider credentials are referenced through a secret manager, support overlap rotation, and never appear in logs, API responses, or business event payloads.
- Outbound calls use stable idempotency keys; signed inbound webhooks are durably received, deduplicated, unordered, and replayable.
- Resend delivery status and suppression handling do not change the underlying commerce transaction.
- The first shipping adapter supports rate quotation, label purchase, cancellation where available, and tracking reconciliation.

## Phase 9 acceptance criteria

- Browser collection accepts only versioned, allowlisted, size-bounded events and never trusts client-supplied commercial outcomes.
- Active engagement time is derived from bounded visible and focused intervals and is explicitly documented as an estimate.
- Trusted Order, Payment, Refund, Fulfillment, and Return facts enter analytics through transactional events with stable event identities.
- Anonymous, session, and Customer identity linking is consent-aware, Store-isolated, reversible through deletion workflows, and free of secrets or payment data.
- Attribution inputs, consent snapshots, policy versions, retention, and destination eligibility are explicit and auditable.
- Analytics queries and exports cannot exhaust the OLTP transaction pool or block commerce commits.
- Meta CAPI and GA4 adapters map canonical events without leaking destination schemas into domain or application code, and equivalent browser and server events are deduplicated.

## Phase 10 acceptance criteria

- Extensibility capabilities have independent authorization scopes, versioned contracts, audit trails, and compatibility tests.
- Store domain resolution cannot broaden Store context and external callbacks defend against SSRF and credential leakage.
- Generated SDKs and MCP tools reuse application use cases instead of duplicating business logic.

## Completion audit

The roadmap is complete only after a final clean-environment audit maps every criterion in its declared release scope to a test, command output, contract assertion, runtime probe, or operational artifact. Missing or indirect evidence keeps the corresponding phase open.

The final audit is recorded in `docs/completion-audit.md`.
