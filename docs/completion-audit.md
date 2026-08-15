# Delivery Roadmap Evidence Audit

This audit records current evidence and open gates. A passing test for an implemented path does not close a phase when its production scheduler, consumer, provider, security boundary, or retained runtime evidence is missing.

## Phase status

| Phase | Evidence status | Open gate |
| --- | --- | --- |
| 0 — Platform | Complete | — |
| 1 — Identity and Merchant | Complete for the declared phase scope | — |
| 2 — Catalog and Pricing | Complete for the declared phase scope | — |
| 3 — Selling | Complete for the declared phase scope | Recoverable automatic Checkout and reservation expiry closes the final Phase 3 gate. Customer association and Order history belong to Phase 7. |
| 4 — Payments | Complete for the declared phase scope | Store-owned Provider administration, sandbox and Stripe production adapters, possession-bound client handoff, signed webhooks, immutable provider-result persistence, stale processing-lease recovery, and bounded graceful worker drain are verified. |
| 5 — Operations | Partial | Fulfillment and Return consumers plus the security and operational release path are verified; the capacity harness still has no retained production-like 10-minute execution report. |
| 6 — Transaction Hardening | Complete for the declared phase scope | Shopper ownership, stale lease recovery, automatic Checkout and reservation expiry, bounded worker drain, and enforced event-consumer ownership are verified. |
| 7 — Real Checkout | Complete for the declared phase scope | Guest and authenticated Customer checkout, saved addresses, recoverable Customer Order history, admin Order filtering, and immutable commercial snapshots are verified. |
| 8 — Provider Integrations | Complete for the declared phase scope | Stripe, Resend, and EasyPost capabilities are wired through Provider-neutral ports with durable Store-owned evidence, bounded credential handling, idempotent recovery, signed inbound reconciliation where applicable, and recoverable workers. |
| 9 — Analytics and Attribution | Partial | The first-party browser event contract, consent-gated Storefront collection path, active-engagement SDK, recoverable server-side sessionization, versioned Store policy, and automatic retention deletion are delivered. Rate limiting, data-subject deletion and identity unlinking, attribution, trusted commerce facts, isolated reporting storage, and conversion destinations remain open. |
| 10 — Extensibility and Ecosystem | Planned | Acceptance evidence will be added as each capability is implemented. |

## Current Phase 4 evidence

| Criterion | Evidence |
| --- | --- |
| Provider boundary | `PaymentProvider`, `PaymentSecretResolver`, and webhook verification are application ports. Sandbox and Stripe implementations live in infrastructure; Stripe wire and credential types do not enter Sales domain models. |
| Provider administration | ADR 0012 defines immutable provider identity, Store ownership, write-only secret references, and enable/disable behavior. Domain tests validate canonical provider names and secret-manager references. The real-router PostgreSQL matrix covers create, uniqueness conflict, list, secret non-disclosure, update, disabled checkout resolution, and cross-Store denial. |
| Payment state | Domain tests cover ordered Payment Attempt and Refund transitions, immutable provider references, exact settlement currency, and refund bounds. Provider command results bind immutable references before queue completion. The PostgreSQL matrix covers command preparation, payment and refund result persistence, possession-bound client handoff, idempotent creation, and capture-to-Order reconciliation. |
| Delivery reliability | Verified webhooks enter a deduplicated inbox; provider commands enter a transactional outbox. Runtime tests cover concurrent claims, stale lease recovery, capped retry, dead letters, and former-owner rejection. |
| Stripe production adapter | ADR 0013 selects Connect direct charges and records merchant-of-record, funds-flow, fee, refund, dispute, and payout consequences. Real HTTP tests verify PaymentIntent creation, client-secret retrieval, Refund creation, Connect and API-version headers, stable idempotency keys, exact form encoding, timestamp tolerance, and signature mapping. The environment secret adapter accepts only constrained payment-secret references. |
| Credential rotation | Provider updates atomically activate new outbound and webhook references, retain one previous reference with a 24-hour deadline, and do not extend the window for an unchanged update. The narrow webhook lookup returns active then previous only before expiry; verifier tests and the real-router PostgreSQL matrix cover the bounded overlap without returning either reference through the API. |
| Stripe onboarding readiness | The provider-neutral onboarding port keeps Stripe wire types in infrastructure. Stripe account retrieval verifies charge and payout permissions, submitted details, active card payments, outstanding requirements, fee payer, and payment-loss liability. The normalized snapshot and stable blocker codes are persisted; action-required accounts remain disabled. Real HTTP adapter tests cover ready and blocked accounts, platform authentication, version pinning, and omission of the connected-account header. |
| Stripe readiness reconciliation | Enabled accounts receive a 24-hour validity deadline and a six-hour next-check schedule. A cross-Store security-definer claim uses `SKIP LOCKED`, one-minute stale-lease recovery, and capped retry while preserving the last valid assessment on dependency failure. Action-required or expired assessments disable new Payment Attempts. The PostgreSQL router matrix covers success, transient retry, stale-owner rejection, fail-closed expiry, blocker visibility, and recovery. |
| Phase evidence | A clean PostgreSQL 18 bootstrap and all 8 API plus 16 infrastructure integration tests pass with the runtime role. Normal workspace tests, Clippy with warnings denied, formatting, language, and OpenAPI reference checks also pass. |

## Current Phase 5 evidence

| Criterion | Evidence |
| --- | --- |
| Fulfillment | ADR 0014 defines authoritative reconciliation and replay behavior. Domain tests cover partial, complete, and impossible derived quantities. The real-router PostgreSQL matrix covers partial allocation, over-allocation conflict, tracking, shipping, delivery, Order-state projection, stale-lease replay, and source-event transition deduplication. |
| Returns | ADR 0014 defines immutable Order-line allocation and atomic refund coordination. Domain tests cover proportional minor-unit rounding. The real-router PostgreSQL matrix covers request, authorization, receipt, restock disposition, completion, positive inventory ledger entries, Refund linkage, provider-dispatch event creation, and replay without a duplicate Refund. |
| Search | Catalog triggers emit `search.product.changed` transactionally. A multi-instance `SKIP LOCKED` worker idempotently upserts Store-keyed GIN documents, so duplicate events converge. `search.rebuild_store_products` is idempotent. The runtime repository integration test covers event processing, search matches, Store isolation, and rebuilding. |
| Telemetry | `telemetry.rs` exports tracing spans through OTLP/HTTP when configured and flushes on shutdown. Prometheus exposes bounded HTTP labels, checkout conversions, payment failures, reservation conflicts, dependency health, database pool use, queue depth, dead letters, and queue age. Worker logs include `worker_id`; HTTP spans retain request IDs without logging credentials. |
| Capacity | `scripts/capacity-test.sh`, `scripts/capacity.js`, and `docs/capacity.md` define the environment, dataset, duration, concurrency, output, and release thresholds. A dated, production-like result with system measurements must be retained before this gate is complete. |
| Runbooks | `docs/operations-runbook.md` covers migration failure, dependency degradation, queue backlog, webhook replay, credential rotation, rollback, and search rebuild. The retained Phase 5 operations record proves a fresh release-image build, migration job, hardened non-root containers, two-instance health, internal-only metrics, secret-log scan, and a 600-request zero-failure rolling update. |

## Current Phase 6 evidence

| Criterion | Evidence |
| --- | --- |
| Shopper ownership | Signed shopper credentials bind Cart, Checkout, Order, and Payment Attempt lineage to one Store and Sales Channel; runtime-role and real-router tests deny cross-shopper and cross-Store access. |
| Automatic expiry | A database claim function leases due Checkouts across tenants with `SKIP LOCKED`. The expiry worker establishes tenant context, expires the Checkout, releases active tracked-inventory reservations, and appends `reservation_expired` ledger entries in one transaction. |
| Lease recovery | Payment inbox, payment outbox, and Checkout expiry claims recover one-minute-old leases. Integration tests abandon claims, advance the Clock, prove another worker can complete them, and reject the former owner. |
| Shutdown | Every in-process worker stops claiming when draining begins and receives the configured bounded interval to finish before forced cancellation. |
| Event ownership | Every Outbox event type references the immutable `integration.event_consumer_registry`. Payment and Search claims require their declared owner. The real-router PostgreSQL test proves an unowned `return.completed` event remains pending after a Payment Worker batch, appears in `integration.event_consumer_backlog()`, reports no owner and no processed count, and cannot have its registry row changed by the runtime role. |

## Current Phase 7 evidence

| Criterion | Evidence |
| --- | --- |
| Guest identity | Domain tests canonicalize valid email, require optional phones to use E.164, and reject malformed contact data. The Store API requires contact input when creating a Checkout. |
| Address snapshots | Billing addresses are required and shipping addresses are conditionally required for shippable lines. Typed Checkout and Order snapshot tables use Store-scoped composite foreign keys, RLS, bounded text, ISO country constraints, and revoked update/delete privileges. |
| Access and immutability | The real-router PostgreSQL matrix proves invalid contact and missing shipping validation, canonical response data, idempotent replay, Checkout-to-Order copying, cross-shopper not-found behavior, and runtime-role denial of contact/address mutation. |
| Shipping quote and selection | Store-owned services normalize destination countries and use one settlement currency. Possession-bound Storefront quotes expose only active matching services. Checkout revalidates the selected service in its transaction, includes its server-owned amount in the total, and copies the immutable service name, amount, currency, and delivery estimate into the Order. |
| Tax calculation | Store-owned Tax Rules permit one active rule per destination country and store rates as integer basis points. Domain tests prove aggregate half-up rounding, stable line allocation, and tax-inclusive extraction. The real-router matrix covers tax-exclusive addition, tax-inclusive extraction, missing-rule rejection, Checkout-to-Order evidence copying, and runtime denial of snapshot mutation. |
| Promotion calculation | Store-owned automatic and code-triggered Promotions support percentage and fixed values, settlement currency, minimum subtotal, percentage caps, priority, and activation windows. Domain tests prove aggregate rounding, capping, eligibility, and stable allocation. PostgreSQL and real-router matrices prove best-rule application before tax, invalid-code rejection, idempotent replay, and immutable Checkout-to-Order evidence. |
| Recalculation | ADR 0010 defines Checkout creation as the sole recalculation boundary, the resolution order for mutable inputs, idempotent replay behavior, and the rule that configuration changes never mutate an existing Checkout or Order. |
| Authenticated Customer | ADR 0011 defines Store ownership, dual credentials, additive immutable shopper association, and snapshot boundaries. Domain tests cover profile validation; the real-router PostgreSQL matrix associates a Customer, creates a saved address, propagates Customer identity through Checkout and Order, and recovers Order history without the shopper credential. |
| Admin Order discovery | The Admin API provides opaque cursor pagination and optional status, Customer, and canonical email filters. The real-router matrix verifies the combined filter against the Customer-owned Order, while application queries remain account- and Store-scoped under RLS. |

## Current Phase 8 notification evidence

| Criterion | Evidence |
| --- | --- |
| Provider boundary | `EmailProvider`, `EmailWebhookVerifier`, and `EmailDeliveryRepository` are application ports. Resend HTTP and Svix-compatible signing details remain in infrastructure. Authentication and commerce use cases depend only on provider-neutral messages and results. |
| Transactional requests | Both administrative confirmation and payment-capture reconciliation insert one `order.confirmed` delivery in the same transaction as the immutable Order transition. The real-router PostgreSQL matrix asserts the canonical recipient, template key and version, Order identity, and commercial snapshot. Authentication links bypass the general queue so reusable sign-in tokens never enter notification persistence. |
| Reliable delivery | Cross-Store security-definer claims use `SKIP LOCKED`, one-minute stale recovery, stable `notification-<delivery_id>` idempotency, capped exponential retry, permanent-failure classification, and dead letters after eight attempts. The runtime-role test proves stale-owner rejection, recovery, dead-letter exhaustion, suppression before claim, and cross-account RLS. |
| Signed reconciliation | The Resend verifier authenticates `svix-id`, `svix-timestamp`, and the exact raw body with a five-minute tolerance before JSON parsing. The 64 KiB HTTP route, OpenAPI contract, verifier tests, and PostgreSQL test cover valid signatures, missing or invalid signatures, duplicate event identity, unknown delivery handling, delivery status, and complaint suppression. |
| Suppression and isolation | Permanent bounces, complaints, and provider suppression create Store-scoped records. Deterministic terminal precedence tolerates unordered callbacks; later delivery events cannot overwrite bounced, complained, or suppressed state. No notification path updates a commerce aggregate. |
| Operations | Prometheus exports bounded pending, processing, dead-letter, suppressed, and oldest-pending gauges. The operations runbook covers lease recovery, provider rate limits, audited replacement requests, and Store-scoped suppression remediation. A clean PostgreSQL 18 bootstrap, all 8 API and 17 infrastructure database tests, normal workspace tests, Clippy, formatting, language, and OpenAPI reference checks pass. |

## Current Phase 8 shipping evidence

| Criterion | Evidence |
| --- | --- |
| Provider boundary | `ShippingProvider` and `ShippingSecretResolver` are capability-specific application ports. EasyPost HTTP, authentication, units, decimal rate conversion, and wire types remain in infrastructure. ADR 0015 records the provider decision and recovery semantics. |
| Provider administration | Store-owned Shipping Provider Accounts bind one Provider, a default origin, enablement, and an opaque credential reference. Owner/administrator authorization, RLS, composite Store ownership, idempotent writes, unique Store/Provider configuration, and non-disclosing list/detail responses are enforced. |
| Credential rotation | Updating a changed outbound credential reference atomically activates it and retains the prior reference with a 24-hour expiry. Repeating the same reference does not extend the overlap window; neither reference is selected by API read queries or serialized responses. |
| Rate and label evidence | Quote requests bind one pending Fulfillment, enabled Store Provider Account, immutable Order destination, Provider origin, parcel, fingerprint, and stable operation identity. Normalized Rates expire after 24 hours. Label purchase persists an operation before calling EasyPost, retrieves the Shipment before every uncertain retry, and atomically stores provider references, tracking, HTTPS label evidence, and the shipped Fulfillment transition. |
| Cancellation | Cancellation has a separate idempotency fingerprint and records `submitted`, `cancelled`, `rejected`, or `not_available`. Reconciliation reads current EasyPost Shipment state before another refund request, and no cancellation result changes Fulfillment state. |
| Tracking reconciliation | A cross-Store security-definer claim uses `SKIP LOCKED`, one-minute stale recovery, stable Label identity, capped backoff, eight-attempt dead letters, and stale-owner rejection. Provider calls occur outside transactions. A delivered observation atomically records tracking evidence, advances only a shipped Fulfillment, and emits its transactional event; unknown states fail closed. |
| Contract and runtime evidence | The Admin OpenAPI contract exposes Provider administration, quotation, label purchase, and cancellation without Provider references or credentials. Real-router PostgreSQL coverage proves quote and label replay, persisted evidence, recovery without a second buy, cancellation status, stale tracking-lease recovery, stale-owner rejection, and delivery transition. EasyPost mock HTTP coverage exercises create, buy, Shipment reconciliation, refund, and Tracker retrieval. |

## Current Phase 9 evidence

| Criterion | Evidence |
| --- | --- |
| Event contract | The Analytics domain allowlists six version-1 browser observations with typed properties. Browser payloads contain no amount, currency, Order status, Payment status, contact, address, token, or free-form object fields. Domain tests enforce consent-policy syntax, path constraints, quantity limits, and a 60-second engagement interval cap. |
| Collection boundary | `POST /store/v1/analytics/events` requires the independent `analytics:write` publishable-key scope, accepts 1-20 events in at most 32 KiB, rejects more than 24 hours of past skew or 5 minutes of future skew, and applies server-owned collection policy `builtin-v1`. Events without analytics-storage consent are acknowledged but not persisted. |
| Storefront SDK | `@chaos-commerce/storefront-analytics` is a dependency-free ES module. It persists random anonymous and tab-session UUIDs, never transmits before storage consent, drops unsent work on revocation, emits only semantic allowlisted events, strips full referrer URLs, measures only visible-and-focused time, splits delayed activity into intervals of at most 60 seconds, flushes SPA navigation and page exit when possible, and retries transient batches with stable event IDs through a bounded memory queue. Six deterministic Node tests cover these behaviors. |
| Persistence and isolation | `analytics.behavior_events` stores canonical client event identity, Store and Sales Channel context, consent evidence, typed JSON properties, occurrence and receipt times, and a 30-day expiry. Store-scoped uniqueness makes replay idempotent without cross-Store collisions. RLS, composite foreign keys, and immutable runtime privileges protect the append-only evidence. |
| Sessionization | Every accepted raw event receives a durable processing row in the same transaction. Multi-instance workers claim at most 100 rows with `SKIP LOCKED`, recover leases after one minute, retry with capped backoff, reject former owners, and expose pending, processing, dead-letter, and age metrics. Store- and Channel-scoped sessions use a 30-minute inactivity boundary, merge out-of-order bridge events under an identity advisory lock, retain event-type counts, and cap estimated active engagement at four hours. |
| Policy and retention | The Admin contract exposes the effective policy and idempotently creates immutable `store-vN` versions. The conservative virtual default enables behavior collection for 30 days while disabling advertising export and identity linking. Collection resolves the effective policy under the authenticated Store and Channel, distinguishes consent and policy discard counts, and stamps the applied version. A shorter policy atomically shortens existing raw and session expiry without extending prior retention. A bounded cross-Store maintenance function removes expired evidence and dependent processing rows every minute, with backlog gauges and deletion counters. |
| Contract and runtime evidence | The Admin and Store OpenAPI 3.1 contracts document policy administration and every collection result. The real-router PostgreSQL matrix proves default resolution, versioned idempotent updates, invalid-retention rejection, policy discard, seven-day persistence, retention shortening and deletion, storage, consent discard, replay deduplication, scope denial, event-property rejection, same-ID Store isolation, cross-account denial and RLS, out-of-order session convergence, engagement aggregation, stale-lease recovery, and stale-owner rejection on a clean PostgreSQL 18 bootstrap. |
| Open gates | Collection rate limiting, data-subject erasure and identity-unlinking jobs, attribution, trusted commerce-fact ingestion, isolated reporting storage, and Meta CAPI/GA4 adapters are not yet delivered. |

## Required release commands

Run from a clean PostgreSQL 18 database with the production extensions preloaded:

```text
DATABASE_URL=... cargo run -p chaos-api --bin chaos-migrate
TEST_DATABASE_URL=... cargo test --workspace -- --ignored --test-threads=1
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
scripts/check-language.sh
```

The OpenAPI unit tests parse every contract and resolve all local references. The database tests use the runtime role and include cross-account or cross-Store denial, concurrency, idempotency, and queue claims.
