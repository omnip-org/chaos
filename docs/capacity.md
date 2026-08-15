# Capacity Baseline

Run `scripts/capacity-test.sh` against a production-like environment and commit its output directory under `capacity-results/` with the release evidence. The seed Store must contain at least 1,000 published products, an active SG Shipping Service, an active SG Tax Rule, and sufficient inventory for the selected tracked and shippable Variant. The publishable key requires `carts:write` and `checkout:write`.

Release thresholds are: HTTP p95 below 250 ms and p99 below 750 ms; at least 50 successful checkout requests per second per API instance; database pool utilization below 80%; no outbox job older than 60 seconds; zero dead letters; and no failed requests during a rolling update. Test with 50 virtual users for 10 minutes after a 2-minute warm-up. Increase the dataset and worker backlog independently to locate database, API, and queue saturation.

The script uses two independent k6 scenarios: a 2-minute warm-up and a 10-minute measurement window. Measurement thresholds exclude warm-up traffic. Each iteration creates a shopper session and possession-bound Cart, propagates the shopper token, writes one line, and creates a Checkout with complete contact, address, and shipping input.

For a disposable environment, `scripts/capacity-seed.sql` creates the minimum production-shaped fixture and prints the selected identifiers. It intentionally bypasses application use cases and must never run against a shared or persistent database. Generate a valid test publishable key, pass only its SHA-256 digest and non-secret metadata to psql, and retain the plaintext outside the repository for the k6 process.

The threshold is a release floor, not a sizing promise. The generated evidence includes the k6 summary, before-and-after Prometheus snapshots, revision, dataset size, API instance count, concurrency, and durations. Add CPU, memory, PostgreSQL pool configuration, and Redis latency observations to a `notes.md` file in the same result directory. Do not commit publishable keys, shopper tokens, or other credentials.

Example:

```bash
BASE_URL=https://capacity.example.com \
METRICS_URL=https://capacity-api-blue.internal \
PUBLISHABLE_KEY=pk_live_redacted \
PRODUCT_VARIANT_ID=019... \
SHIPPING_SERVICE_ID=019... \
DEPLOYMENT_REVISION=f65bd1d \
DATASET_SIZE=1000 \
API_INSTANCES=2 \
scripts/capacity-test.sh
```
