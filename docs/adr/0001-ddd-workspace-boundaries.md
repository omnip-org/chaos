# ADR 0001: Enforce DDD Boundaries with a Cargo Workspace

- Status: Accepted
- Date: 2026-08-14

## Decision

Use four Cargo packages: `chaos-domain`, `chaos-core`, `chaos-api`, and `chaos-worker`. `chaos-core` is the modular-monolith business core: each capability keeps its use cases, PostgreSQL persistence, runtime adapters, and only its real external seams together. Dependencies may point from the delivery packages toward the core and from the core toward the domain. The domain package must not depend on Axum, SQLx, Redis, or Serde. MCP is an API delivery boundary under `chaos-api/src/mcp`; the independently deployed Worker has its own composition package.

## Rationale

Keeping application and infrastructure in separate Cargo packages made every repository operation cross a port and encouraged transaction abstractions with only one implementation. The core package keeps the compile-time boundary at HTTP/Worker versus business core, while bounded-context modules keep use cases and persistence close enough to make direct, concrete database calls readable.

## Consequences

The domain remains independently testable and free of infrastructure dependencies. External systems such as Stripe, object storage, identity providers, clocks, and rate limiters still use small interfaces; ordinary PostgreSQL repositories do not gain an interface merely for layering.
