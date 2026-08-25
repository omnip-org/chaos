# ADR 0029: Use PGMQ for Durable Delivery

- Status: Accepted
- Date: 2026-08-20

## Context

Chaos runs HTTP API replicas separately from an independently scalable Worker.
Payment commands, Email notifications, fulfillment projections, search
refreshes, provider webhooks, and external provider delivery all need crash recovery and safe
concurrent consumption. Maintaining a separate status, attempt counter,
availability timestamp, and lease implementation for each flow duplicated the
same queue mechanism and made reliability harder to audit.

PostgreSQL is already the durable source of truth and the deployment includes
PGMQ. Adding a separate broker would increase operational cost without solving a
current scale constraint.

## Decision

Use logged PGMQ queues for event delivery:

- `chaos_payment_commands`;
- `chaos_email_commands`;
- `chaos_shipping_commands`;
- `chaos_search_events`;
- `chaos_webhooks`;
- `chaos_analytics_deliveries`.

The authoritative delivery row remains in the `integration` schema: outbound
events use `integration.event_outbox`, while every verified provider webhook
uses `integration.provider_webhook_inbox`. Commerce tables remain the source of truth
for order/payment/fulfillment state transitions. Each integration row keeps the
business payload, stable event or delivery identifier, processing outcome, and bounded error. A `BEFORE
INSERT` trigger sends a versioned message containing only that row identifier
and stores the returned PGMQ message ID. Claim routines join the message back
to the authoritative row. Finish routines update the row and delete the
message in one database transaction, or change its visibility timeout for a
bounded exponential retry. PGMQ `read_ct` is the attempt count.

Claims also delete envelopes whose authoritative row no longer exists or is
already terminal. This handles Store deletion and administrative cleanup without
leaving invisible messages to cycle forever. Queue state remains in PGMQ and
authoritative integration rows; no second message archive is maintained.

Application ports describe capability-specific jobs; PGMQ remains an
infrastructure detail. The runtime role has no direct PGMQ privileges and calls
only reviewed routines in the `integration` schema. API replicas do not consume
queues.

Business outbox routing is stored in `integration.event_routes`: each internal
event type points directly to its PGMQ queue. The database routine only resolves that
registered value, so adding a consumer does not require changing a routing
`CASE` expression.

Delivery is at least once across an external Provider call: a process can stop
after the Provider succeeds but before the database deletes the message.
Consumers therefore use stable Provider idempotency keys, stable Meta event IDs,
or transactional domain guards. PGMQ visibility does not imply exactly-once
external side effects.

Provider-facing consumers claim at most ten messages per batch. Provider request
timeouts are bounded to ten seconds and queue visibility is two minutes, so one
Worker normally finishes a sequential batch before another Worker can reclaim
it. Stable idempotency still protects the crash boundary and unusual latency.

Scheduled reconciliation derived from authoritative current state, such as
shipment tracking, cancellation, and checkout expiry, continues to use short
`FOR UPDATE SKIP LOCKED` row leases. Creating synthetic
queue messages for every future due time would add state without improving the
invariant.

## Consequences

- Multiple Worker replicas can share queues without process-local coordination.
- Queue visibility, attempt counting, and retry scheduling have one implementation.
- Delivery rows remain queryable business evidence rather than becoming an
  opaque message log.
- Database recreation must recreate the named queues; PGMQ extension upgrades
  require their own migration review.
- Completed PGMQ envelopes are deleted instead of archived because the
  authoritative integration row already retains the audit evidence. Terminal
  failures remain visible there as dead letters without growing duplicate PGMQ
  archive tables.

This ADR supersedes the custom event-queue lease mechanics described in ADRs
0002, 0007, 0014, 0025, and 0026. Their deployment and domain decisions remain
in force.
