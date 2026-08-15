# Capacity Baseline

Run `scripts/capacity-test.sh` against a production-like environment and retain its JSON summary with the release evidence. The seed Store must contain at least 1,000 published products and sufficient inventory.

Release thresholds are: HTTP p95 below 250 ms and p99 below 750 ms; at least 50 successful checkout requests per second per API instance; database pool utilization below 80%; no outbox job older than 60 seconds; zero dead letters; and no failed requests during a rolling update. Test with 50 virtual users for 10 minutes after a 2-minute warm-up. Increase the dataset and worker backlog independently to locate database, API, and queue saturation.

The threshold is a release floor, not a sizing promise. Record CPU, memory, pool size, Redis latency, queue age, API latency, checkout throughput, deployment revision, dataset size, and test command so results are reproducible.
