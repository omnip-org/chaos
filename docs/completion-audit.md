# Delivery Roadmap Completion Audit

## Phase 5 evidence

| Criterion | Evidence |
| --- | --- |
| Fulfillment | Domain transition tests in `chaos-domain`; the real-router PostgreSQL matrix covers partial allocation, over-allocation conflict, tracking, shipping, and delivery; `fulfillment.*` outbox events reconcile downstream Order projections. |
| Returns | Domain transition tests and the real-router PostgreSQL matrix cover request, authorization, receipt, restock disposition, completion, positive inventory ledger entry, and one `return.completed` refund-coordination event. |
| Search | Catalog triggers emit `search.product.changed` transactionally. A multi-instance `SKIP LOCKED` worker idempotently upserts Store-keyed GIN documents, so duplicate events converge. `search.rebuild_store_products` is idempotent. The runtime repository integration test covers event processing, search matches, Store isolation, and rebuilding. |
| Telemetry | `telemetry.rs` exports tracing spans through OTLP/HTTP when configured and flushes on shutdown. Prometheus exposes bounded HTTP labels, checkout conversions, payment failures, reservation conflicts, dependency health, database pool use, queue depth, dead letters, and queue age. Worker logs include `worker_id`; HTTP spans retain request IDs without logging credentials. |
| Capacity | `scripts/capacity-test.sh`, `scripts/capacity.js`, and `docs/capacity.md` define the environment, dataset, duration, concurrency, output, and release thresholds. Existing rolling-update smoke tooling verifies readiness and zero-downtime behavior. |
| Runbooks | `docs/operations-runbook.md` covers migration failure, dependency degradation, queue backlog, webhook replay, credential rotation, rollback, and search rebuild. |

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
