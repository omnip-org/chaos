# Operations Runbook

## Migration failure

Stop the rollout before shifting traffic. Preserve the failed database and migration logs, compare the migration checksum with the release artifact, and restore from the latest tested backup only when the migration is not forward-fixable. Never edit an applied migration. Validate a corrected additive migration on a production-sized clone before resuming blue then green.

## Dependency degradation

Use `/health/ready` and `chaos_dependencies_healthy` to identify PostgreSQL or Redis loss. Remove unready instances from rotation. PostgreSQL errors require write traffic to remain stopped until consistency checks pass. Redis-backed authentication ceremonies may be retried; do not bypass authentication or weaken timeouts.

## Queue backlog

Alert when `chaos_outbox_oldest_pending_seconds` exceeds 60 seconds, any dead letter exists, or pending work grows for 10 minutes. Confirm provider health, database pool saturation, and worker logs by `worker_id`. Processing rows older than one minute are automatically reclaimed by another worker; repeated lease expiry on the eighth attempt moves the row to `dead_letter`. Scale workers only after confirming downstream capacity. Replay dead letters with a new audited event; never mutate payloads in place.

Query `integration.event_consumer_backlog()` to separate owned delivery failures from deliberately unowned event types. An unowned row has a null `consumer_owner` and must remain pending; it is not reconciled. Assigning a consumer requires a reviewed migration that updates the immutable registry and ships the idempotent consumer in the same release. Runtime roles cannot edit ownership declarations.

## Webhook replay

Verify the provider signature and event identifier against provider records. Durable inbox uniqueness makes exact replay safe. Reset only a confirmed failed inbox row to pending in a reviewed transaction, retain its attempt history, and monitor the associated aggregate to completion.

## Notification delivery

Alert on `chaos_notification_email_oldest_pending_seconds`, `chaos_notification_email_dead_letter`, and sustained growth in `chaos_notification_email_pending`. Check Resend availability and rate limits before scaling workers. A processing delivery is reclaimed after one minute and retains the same `notification-<delivery_id>` provider idempotency key. Do not edit a template payload or provider message identifier in place; create an audited replacement request when a permanent failure is corrected.

For complaints, permanent bounces, or `email.suppressed`, confirm the signed webhook exists in `notification.webhook_events` and the Store-scoped recipient appears in `notification.email_suppressions`. A suppression prevents future claims for that Store but never changes commerce state. Manual removal requires verified recipient remediation and an audited administrative procedure. Never remove another Store's suppression based on a shared email address.

## Checkout expiry backlog

Due Checkouts are claimed in bounded batches with one-minute recoverable leases. Each completion expires the Checkout and, when an active tracked-inventory reservation exists, releases its quantity and records `reservation_expired` in the inventory ledger in the same transaction. If a worker exits after claiming, another instance reclaims the Checkout after the lease timeout. Investigate database pool saturation and worker logs by `worker_id` when expired pending Checkouts are older than two minutes. Do not close Checkouts or edit reserved balances independently; replay the normal expiry worker after correcting the underlying fault.

## Credential rotation

Create replacement values in the external secret manager, then update the Provider account with their opaque references. The outbound credential changes immediately and retains a 24-hour rollback deadline. Webhook verification tries the active reference first and accepts the immediately previous reference for the same 24-hour overlap. Repeating the update with unchanged references does not extend these deadlines. Confirm the deadlines through the Admin API, validate outbound calls with the new credential and inbound signatures with both webhook secrets, then revoke the previous external values after expiry. A later rotation replaces the previous references, so do not start another rotation before the current window closes. Never log plaintext secrets or secret references. Record owner, issue time, expiry, validation, and revocation evidence.

Shipping Provider Accounts use the same outbound rotation discipline but have no inbound secret. Update the Store account with the replacement `env://CHAOS_SHIPPING_SECRET_*` reference, verify the returned rollback deadline without expecting either reference in the response, and exercise a non-destructive rate quote before revoking the previous EasyPost key. Keep the account disabled if the deployment does not contain the named adapter or the default origin is no longer operationally valid.

## Payment Provider readiness

Store Provider configuration as disabled until external onboarding is expected to be complete, then request enablement through the Admin API. For Stripe, `action_required` blocker codes identify disabled charges or payouts, incomplete details, inactive card payments, outstanding requirements, or a fee/loss responsibility mismatch. Remediate the connected Account in Stripe and repeat the enable request. Do not bypass readiness in PostgreSQL: the selected direct-charge model requires the connected account to pay Stripe fees and carry payment-loss liability. Treat Stripe unavailability as a dependency incident and retry without changing credential references.

Enabled accounts reconcile every six hours and assessments expire after 24 hours. Dependency failures retain the last valid assessment and retry with capped exponential backoff; abandoned claims are reclaimed after one minute. Alert on `chaos_payment_provider_readiness_retrying`, `chaos_payment_provider_readiness_expiring`, and `chaos_payment_provider_action_required`; inspect `readiness_last_error` under the affected merchant context. `readiness_expired` is fail-closed and requires a successful Admin enable request after the dependency or connected Account is repaired.

## Rollback

Shift traffic to the healthy adjacent version with `scripts/rolling-update.sh`. Application rollback is allowed only while database changes remain backward compatible. Otherwise ship a forward fix. Confirm readiness, error rate, queue age, checkout success, and payment failures before declaring recovery.

## Search rebuild

Run `SELECT search.rebuild_store_products(account_id, store_id)` as the runtime role with `app.merchant_account_id` set. Compare the returned count with Catalog products, then test Storefront `q` results. The operation is idempotent and Store scoped.
