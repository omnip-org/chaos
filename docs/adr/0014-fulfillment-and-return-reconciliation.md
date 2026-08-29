# ADR 0014: Fulfillment and Return Reconciliation

- Status: Proposed
- Date: 2026-08-16

> This ADR describes future behavior and is not implemented in the current codebase. Current Fulfillment supports manual create, shipped, delivered, and cancelled transitions. Returns and return reconciliation remain future work; this document must not be used as current behavior documentation.

## Context

Fulfillment transitions and completed Returns commit before downstream Order projections and payment-provider work. Delivery can therefore be duplicated, delayed, reordered, or interrupted after a worker lease is acquired. Return refunds also need a deterministic amount that cannot drift with later catalog or pricing changes.

## Decision

The fulfillment bounded context owns a recoverable consumer for `fulfillment.shipped`, `fulfillment.delivered`, `fulfillment.cancelled`, and `return.completed`.

Order fulfillment and delivery states are derived from persisted Order lines, Fulfillments, and Fulfillment lines. Event payloads identify the aggregate but are not trusted as commercial or state authority. Each consumed Fulfillment event records an append-only transition keyed by its source event. Reprocessing recalculates current state and cannot duplicate that transition.

Return refund amounts are allocated when the Return is requested from immutable Order-line totals. Partial quantities use proportional minor-unit allocation; the final accepted quantity receives the remaining minor units. Completing a Return causes the consumer to lock it, create one Refund against a captured Payment Attempt, link that Refund to the Return, and publish `refund.create_requested` in one transaction. A linked Refund makes replay a no-op.

The application layer defines the worker port and orchestration. PostgreSQL claiming, reconciliation queries, and payment persistence remain infrastructure concerns. The sales and payments domains do not depend on fulfillment infrastructure.

## Consequences

- Order responses expose separate commercial, fulfillment, and delivery states.
- Queue replay and stale-lease recovery converge from authoritative data.
- A completed zero-value Return needs no provider Refund and retains a null `refund_id`.
- A Return cannot coordinate a Refund until a captured Payment Attempt exists and sufficient captured value remains; repeated failure follows the bounded retry and dead-letter policy.
- Shipping charges and discretionary adjustments are not automatically refunded by product Return allocation. Operators may issue a separate Refund when policy requires it.
