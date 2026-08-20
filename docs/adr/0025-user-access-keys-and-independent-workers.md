# ADR 0025: Use User-owned Access Keys and Independent Workers

- Status: Accepted
- Date: 2026-08-20

## Context

Users may operate multiple Stores. A Store-scoped secret key forces an AI client to manage one administrative credential per Store and attributes mutations to the person who created the Store Key rather than the User whose AI client made the request. Running every background poller in every API replica also couples HTTP capacity to database polling capacity.

## Decision

Private Access Keys belong to Users in the Identity context. Authentication resolves `access_key_id` and `user_id`. Each MCP request explicitly selects a Store, rechecks current membership, and carries the Key identity into the Store actor used by application use cases. Stores issue only Publishable Sales Channel Keys with storefront capabilities.

The HTTP API retains only identity bootstrap, storefront and channel operations, Provider webhooks, health, and metrics. Store administration is provided through MCP tools.

Background polling runs in the independently deployed `chaos-worker` binary. API replicas do not start Workers. Queue consumers remain lease-based, idempotent, and safe with multiple Worker replicas; no correctness invariant relies on running exactly one process.

## Consequences

Revoking a User Key disables every MCP connection that presents it. Removing a Store membership immediately removes that Key's access to the Store. Observability can correlate request, Access Key, User, and Store identities. API and Worker replica counts can be tuned independently, and API rollouts no longer interrupt background consumers.
