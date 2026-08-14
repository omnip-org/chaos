# ADR 0001: Enforce DDD Boundaries with a Cargo Workspace

- Status: Accepted
- Date: 2026-08-14

## Decision

Use four Cargo packages: `chaos-domain`, `chaos-application`, `chaos-infrastructure`, and `chaos-api`. Dependencies may point only from outer layers toward inner layers. The domain package must not depend on Axum, SQLx, Redis, or Serde.

## Rationale

Directory conventions cannot prevent handlers from querying a database directly or domain objects from depending on transport DTOs. Cargo packages enforce dependency direction at compile time while preserving a single modular-monolith deployment unit.

## Consequences

Types must be mapped explicitly across boundaries, which adds a small amount of code. In return, business rules remain independently testable, adapters are replaceable, and future bounded-context extraction has a lower migration cost.
