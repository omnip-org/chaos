# Phase 5 Operations Evidence — 2026-08-16

## Scope

This record covers local release-path and capacity-harness smoke evidence for revision `f65bd1d` plus the pending Dockerfile and harness corrections. It is not the retained production-like 10-minute capacity baseline and does not close that gate.

## Image and deployment path

- The initial release image build failed because the builder did not copy the compile-time OpenAPI inputs. Adding `COPY openapi ./openapi` made the release binaries build successfully.
- A fresh PostgreSQL 18 migration job completed in an isolated Compose project.
- Both API instances and the Caddy gateway reached healthy state.
- API containers run as UID/GID `10001:10001` with a read-only root filesystem, all Linux capabilities dropped, and `no-new-privileges` enabled.
- `scripts/rolling-update.sh` replaced blue and then green while a concurrent probe sent 600 requests through the gateway.
- Probe result: `requests=600 failures=0`.
- The gateway returned 404 for `/metrics`, while the internal API endpoint returned 200.
- A bounded log scan found no API key, shopper-token header, Stripe client-secret, or Stripe secret-key patterns.

## Capacity-harness smoke

The disposable Store contained 1,000 active and published Products with prices, an active SG Tax Rule, an active SG Shipping Service, and one tracked Variant with 1,000,000 units on hand. The corrected scenario used 50 virtual users, a 5-second warm-up, and a 15-second measurement window against two API instances.

- Successful Checkout flow rate: 223.49 per second.
- HTTP p95: 204.20 ms.
- HTTP p99: 427.65 ms.
- Failed HTTP requests: 0%.
- Successful checks: 100% across 6,194 iterations.

The run exposed and led to correction of two stale harness assumptions: shopper-session bootstrap was missing, and dynamic resource IDs were creating high-cardinality k6 URL series. The final script groups dynamic URLs under bounded `name` tags.

## Remaining gate

Run the final 2-minute warm-up plus 10-minute measurement in a production-like environment, retain the generated summaries and Prometheus snapshots, add CPU, memory, pool, and Redis observations, and exercise a rolling update during the measurement window.
