# Delivery Roadmap Evidence Audit

This audit records current evidence and open gates. A passing test for an implemented path does not close a phase when its production scheduler, consumer, provider, security boundary, or retained runtime evidence is missing.

## Phase status

| Phase | Evidence status | Open gate |
| --- | --- | --- |
| 0 — Platform | Complete | — |
| 1 — Identity and Merchant | Complete for the declared phase scope | — |
| 2 — Catalog and Pricing | Complete for the declared phase scope | — |
| 3 — Selling | Complete for the declared phase scope | Recoverable automatic Checkout and reservation expiry closes the final Phase 3 gate. Customer association and Order history belong to Phase 7. |
| 4 — Payments | Partial | The sandbox flow, stale processing-lease recovery, and bounded graceful worker drain are verified; provider administration and a live provider adapter remain open. |
| 5 — Operations | Partial | Fulfillment and Return events are emitted but do not yet have downstream reconciliation consumers; the capacity harness has no retained production-like execution report. |
| 6 — Transaction Hardening | Partial | Shopper ownership, stale lease recovery, automatic Checkout and reservation expiry, and bounded worker drain are implemented; declared event-consumer ownership remains open. |
| 7–10 | Planned | Acceptance evidence will be added as each capability is implemented. |

## Current Phase 5 evidence

| Criterion | Evidence |
| --- | --- |
| Fulfillment | Domain transition tests in `chaos-domain`; the real-router PostgreSQL matrix covers partial allocation, over-allocation conflict, tracking, shipping, and delivery. `fulfillment.*` events are written transactionally, but downstream Order reconciliation remains open. |
| Returns | Domain transition tests and the real-router PostgreSQL matrix cover request, authorization, receipt, restock disposition, completion, and positive inventory ledger entries. `return.completed` is written transactionally, but refund coordination remains open. |
| Search | Catalog triggers emit `search.product.changed` transactionally. A multi-instance `SKIP LOCKED` worker idempotently upserts Store-keyed GIN documents, so duplicate events converge. `search.rebuild_store_products` is idempotent. The runtime repository integration test covers event processing, search matches, Store isolation, and rebuilding. |
| Telemetry | `telemetry.rs` exports tracing spans through OTLP/HTTP when configured and flushes on shutdown. Prometheus exposes bounded HTTP labels, checkout conversions, payment failures, reservation conflicts, dependency health, database pool use, queue depth, dead letters, and queue age. Worker logs include `worker_id`; HTTP spans retain request IDs without logging credentials. |
| Capacity | `scripts/capacity-test.sh`, `scripts/capacity.js`, and `docs/capacity.md` define the environment, dataset, duration, concurrency, output, and release thresholds. A dated, production-like result with system measurements must be retained before this gate is complete. |
| Runbooks | `docs/operations-runbook.md` covers migration failure, dependency degradation, queue backlog, webhook replay, credential rotation, rollback, and search rebuild. |

## Current Phase 6 evidence

| Criterion | Evidence |
| --- | --- |
| Shopper ownership | Signed shopper credentials bind Cart, Checkout, Order, and Payment Attempt lineage to one Store and Sales Channel; runtime-role and real-router tests deny cross-shopper and cross-Store access. |
| Automatic expiry | A database claim function leases due Checkouts across tenants with `SKIP LOCKED`. The expiry worker establishes tenant context, expires the Checkout, releases active tracked-inventory reservations, and appends `reservation_expired` ledger entries in one transaction. |
| Lease recovery | Payment inbox, payment outbox, and Checkout expiry claims recover one-minute-old leases. Integration tests abandon claims, advance the Clock, prove another worker can complete them, and reject the former owner. |
| Shutdown | Every in-process worker stops claiming when draining begins and receives the configured bounded interval to finish before forced cancellation. |
| Event ownership | Open. Event types and their responsible consumers still require an explicit registry plus evidence that unowned events remain visible and unreconciled. |

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
