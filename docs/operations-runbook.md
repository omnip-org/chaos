# Operations Runbook

## Migration failure

Stop the rollout before shifting traffic. Preserve the failed database and migration logs, compare the migration checksum with the release artifact, and restore from the latest tested backup only when the migration is not forward-fixable. Never edit an applied migration. Validate a corrected additive migration on a production-sized clone before resuming blue then green.

## Dependency degradation

Use `/health/ready` and `chaos_dependencies_healthy` to identify PostgreSQL or Redis loss. Remove unready instances from rotation. PostgreSQL errors require write traffic to remain stopped until consistency checks pass. Redis-backed authentication ceremonies may be retried; do not bypass authentication or weaken timeouts.

## Queue backlog

Alert when `chaos_outbox_oldest_pending_seconds` exceeds 60 seconds, any dead letter exists, or pending work grows for 10 minutes. Confirm provider health, database pool saturation, and worker logs by `worker_id`. Processing rows older than one minute are automatically reclaimed by another worker; repeated lease expiry on the eighth attempt moves the row to `dead_letter`. Scale workers only after confirming downstream capacity. Replay dead letters with a new audited event; never mutate payloads in place.

## Webhook replay

Verify the provider signature and event identifier against provider records. Durable inbox uniqueness makes exact replay safe. Reset only a confirmed failed inbox row to pending in a reviewed transaction, retain its attempt history, and monitor the associated aggregate to completion.

## Checkout expiry backlog

Due Checkouts are claimed in bounded batches with one-minute recoverable leases. Each completion expires the Checkout and, when an active tracked-inventory reservation exists, releases its quantity and records `reservation_expired` in the inventory ledger in the same transaction. If a worker exits after claiming, another instance reclaims the Checkout after the lease timeout. Investigate database pool saturation and worker logs by `worker_id` when expired pending Checkouts are older than two minutes. Do not close Checkouts or edit reserved balances independently; replay the normal expiry worker after correcting the underlying fault.

## Credential rotation

Create a replacement API key or webhook secret, deploy it to every instance, validate both paths during the overlap window, then revoke the previous credential. Never log plaintext secrets. Record owner, issue time, expiry, and revocation evidence.

## Rollback

Shift traffic to the healthy adjacent version with `scripts/rolling-update.sh`. Application rollback is allowed only while database changes remain backward compatible. Otherwise ship a forward fix. Confirm readiness, error rate, queue age, checkout success, and payment failures before declaring recovery.

## Search rebuild

Run `SELECT search.rebuild_store_products(account_id, store_id)` as the runtime role with `app.merchant_account_id` set. Compare the returned count with Catalog products, then test Storefront `q` results. The operation is idempotent and Store scoped.
