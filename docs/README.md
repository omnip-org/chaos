# Repository Guide

This page is the shortest path from a product change to the code that owns it.
Keep product language consistent with [`product-model.md`](product-model.md), and
preserve the dependency direction in
[`adr/0001-ddd-workspace-boundaries.md`](adr/0001-ddd-workspace-boundaries.md).

## Documentation map

| Document | Authority |
| --- | --- |
| [`product-model.md`](product-model.md) | Current product terms and ownership boundaries |
| [`architecture.md`](architecture.md) | Current runtime, authentication, authorization, and reliability model |
| [`database-conventions.md`](database-conventions.md) | Required schema, SQL, isolation, migration, money, and time rules |
| [`deployment.md`](deployment.md) | Production topology, secrets, bootstrap, rollout, and rollback |
| [`postgresql-extensions.md`](postgresql-extensions.md) | PostgreSQL image and extension lifecycle |
| [`adr/`](adr/) | Historical decisions and their status; an amended ADR must be read with its named successor |

## Runtime entry points

| Runtime | Entry point | Responsibility |
| --- | --- | --- |
| HTTP API | `crates/chaos-api/src/main.rs` | Identity bootstrap, Storefront APIs, Provider webhooks, and health |
| MCP | `crates/chaos-mcp/src/router.rs` | AI-operated Store administration authenticated by User Access Keys |
| Worker | `crates/chaos-api/src/bin/chaos-worker.rs` | Durable polling and Provider reconciliation outside API replicas |
| Migration job | `crates/chaos-api/src/bin/chaos-migrate.rs` | Applies SQL migrations before an application rollout |

## Dependency layers

| Crate | Look here for | Must not own |
| --- | --- | --- |
| `chaos-domain` | Business types, validation, and state transitions | HTTP, SQL, serialization, Provider SDKs |
| `chaos-application` | Use cases and ports | Axum handlers and SQL queries |
| `chaos-infrastructure` | PostgreSQL repositories and external Provider adapters | Transport DTOs and new business rules |
| `chaos-api` | HTTP routes, DTOs, runtime composition, and Worker loops | Direct business persistence |
| `chaos-mcp` | MCP tools and MCP transport | SQL queries |

## Change routing

| Product area | Domain and use cases | Delivery and adapters | Database ownership |
| --- | --- | --- | --- |
| Users, external identity, Access Keys | `identity` | HTTP auth and identity adapter | `identity` |
| Stores, memberships, channels, Publishable Keys | `store` | MCP Store tools and Store repositories | `commerce` |
| Products, variants, collections, media | `catalog` | MCP catalog tools and catalog repositories | `commerce` |
| Prices, promotions, and tax | `pricing` | MCP pricing tools and pricing repositories | `commerce` |
| Stock and reservations | `inventory` | MCP inventory tools and inventory repository | `commerce` |
| Carts, checkout, customers, and orders | `sales` | Storefront HTTP and sales repositories | `commerce` |
| Payments and refunds | `payments` | MCP payment tools and Stripe adapters | `commerce` |
| Shipping, fulfillment, and returns | `fulfillment` | MCP fulfillment tools and shipping adapters | `commerce` |
| Webhooks, outbox, and idempotency | application ports | Worker loops and integration repositories | `integration` |
| Commerce events and external provider delivery | `analytics` | Storefront collection, MCP settings, and Worker delivery | `integration` |

Rust business modules remain useful navigation boundaries; they do not require
matching PostgreSQL schemas.

HTTP delivery code is grouped by public responsibility under
`crates/chaos-api/src/http/`:

- `identity/` contains account bootstrap and User Access Key endpoints;
- `storefront/` contains every publishable Store API surface;
- `webhooks/` contains Provider callback endpoints;
- `operations/` contains health checks;
- `shared/` contains transport extractors, envelopes, OpenAPI, and test support.

MCP delivery keeps protocol concerns separate from commerce capabilities under
`crates/chaos-mcp/src/`:

- `router.rs`, `auth.rs`, `error.rs`, and `mutation.rs` own MCP transport,
  authentication, error mapping, confirmation, and idempotency behavior;
- `tools/mod.rs` owns shared MCP state and is the single tool-router assembly
  point;
- `tools/store/`, `catalog/`, `pricing/`, `operations/`, and `integrations/`
  group tool implementations by product capability.

Moving a tool between capability modules must not rename the public tool or
change its input schema. Protocol-version changes belong in the MCP boundary,
not in application use cases.

## Contracts and operations

- `openapi/` contains the generated or reviewed HTTP contracts.
- `packages/js/` is the Storefront JavaScript client.
- `migrations/0002_identity.sql`, `0003_commerce.sql`, and
  `0004_integration.sql` are the original business-schema bootstrap files;
  later numbered migrations, including integration capability changes, fix
  forward from that baseline. `0001_platform.sql` and
  `0005_runtime_hardening.sql` own platform setup and final grants.
- `deploy/` contains the production-equivalent Compose topology and origin TLS
  certificate used behind Cloudflare.
- `scripts/storefront-demo.mjs` exercises the supported commerce flow.

## Required verification

Run the commands listed in the root `README.md` and `CONTRIBUTING.md`. Database
changes additionally require a disposable PostgreSQL migration run and Store
isolation tests.
