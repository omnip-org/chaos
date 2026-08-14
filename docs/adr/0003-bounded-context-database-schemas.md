# ADR 0003: Organize PostgreSQL by Bounded-Context Schema

- Status: Accepted
- Date: 2026-08-14

## Decision

Use PostgreSQL schemas to express bounded-context ownership. Use shared tables with `tenant_id` and RLS for tenant isolation. Never create one schema or database per merchant tenant in the primary operating model.

Business SQL must use schema-qualified identifiers. PostgreSQL extensions live in `extensions`; SQLx migration metadata remains in `public`; business objects live in their owning context schema.

## Rationale

Bounded-context schemas make ownership visible, reduce accidental name collisions, and provide useful permission boundaries without multiplying migration and connection-pool operations per merchant. A schema per merchant would make global migrations, connection pooling, analytics, and high-tenant-count operations unnecessarily expensive.

## Consequences

Cross-context queries become explicit and may require carefully reviewed cross-schema foreign keys. Runtime roles need `USAGE` grants for each schema. Every repository query must use qualified names, and every new schema must define default privileges for the runtime role.
