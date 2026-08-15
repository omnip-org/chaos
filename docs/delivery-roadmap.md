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
| 3 — Selling | In progress | Shared idempotency records; location-aware stock balances; append-only inventory ledger; concurrency-safe reservation transitions; mutable Storefront Carts bound to one Store, Channel, currency, and Price List; atomic Checkout with immutable commercial snapshots and tracked-inventory reservations; immutable Order snapshots; explicit pending-to-confirmed/cancelled state machine; append-only transition audit trail; atomic reservation consumption/release; Admin and Store API contracts; runtime-role isolation, oversell, concurrency, and immutability tests | Add shopper-level resource ownership and run reservation and Checkout expiry from a recoverable production scheduler. |
| 4 — Payments | In progress | Provider-neutral application port and sandbox adapter; currency-safe Payment Attempt and Refund state machines; immutable provider references; HMAC verification before tenant resolution; deduplicated durable webhook inbox; transactional payment outbox; `SKIP LOCKED` workers; capped exponential retry and dead-letter handling; atomic capture-to-Order/inventory reconciliation; Store, Admin, and Webhook contracts | Recover stale leases, drain workers safely, add Store-owned provider administration, and verify one production payment adapter end to end. |
| 5 — Operations | In progress | Partial Fulfillment and shipment tracking; authorized Returns with receipt disposition and inventory restocking; fulfillment and refund-coordination events; transactional search events; rebuildable Store-isolated search; optional OTLP trace export; bounded business and queue metrics; capacity harness; operations runbooks; Admin and Store contracts | Implement fulfillment and return event consumers, retain an executed capacity report, and close the security and operational release gates. |
| 6 — Transaction Hardening | Planned | Existing idempotency, inbox, outbox, RLS, and state-machine foundations | Shopper access boundaries; lease recovery; deterministic expiry scheduling; event consumer ownership; graceful worker drain; process-crash and replay evidence. |
| 7 — Real Checkout | Planned | Cart, immutable Checkout, Order, Money, Price List, and inventory snapshot foundations | Customer or guest contact; billing and shipping addresses; shipping options and rates; tax; promotions; complete total allocation; customer Order access. |
| 8 — Provider Integrations | Planned | Payment and webhook ports, Fulfillment state, transactional events, and SMTP-based authentication delivery | Stripe adapter and onboarding decision; Resend notification delivery; shipping provider port and first adapter; provider administration, secrets, rotation, signed webhooks, and reconciliation. |
| 9 — Analytics and Attribution | Planned | Transactional commerce events, Store and Channel boundaries, search, immutable snapshots, and queue infrastructure | First-party event contract; Storefront collection SDK; active engagement; consent and retention; sessionization; attribution; isolated analytics read models; Meta CAPI and GA4 destinations. |
| 10 — Extensibility and Ecosystem | Planned | Versioned Admin, Store, and Webhook contracts; scoped API keys; search read model | Store domains; collections and media; localization; outbound webhooks; MCP; generated SDKs; compatibility automation; third-party application workflows. |

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
