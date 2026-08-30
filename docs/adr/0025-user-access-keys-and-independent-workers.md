# ADR 0025: Use User-owned Access Keys and Independent Workers

- Status: Superseded
- Date: 2026-08-20

> Superseded by the OAuth-only MCP authentication model and the current
> checkout lifecycle. Kept as historical context for the independent Worker
> decision.

## Context

Users may operate multiple Stores. A Store-scoped secret key forces an AI client to manage one administrative credential per Store and attributes mutations to the person who created the Store Key rather than the User whose AI client made the request. Running every background poller in every API replica also couples HTTP capacity to database polling capacity.

## Historical decision

Private User-owned Access Keys were proposed for MCP administration. Each MCP
request explicitly selected a Store and rechecked current membership. Stores
issued Publishable Channel Keys for storefront capabilities.

That historical HTTP surface retained identity bootstrap, storefront and channel operations, Provider webhooks, and health checks. Store administration is provided through MCP tools.

Background polling runs in the independently deployed `chaos-worker` binary. API replicas do not start Workers. Queue consumers remain lease-based, idempotent, and safe with multiple Worker replicas; no correctness invariant relies on running exactly one process.

## Historical consequences

The independent Worker decision remains current: API and Worker replica counts
can be tuned independently, and API rollouts do not interrupt background
consumers. The Access Key decision is retired; MCP now uses OAuth access tokens.
