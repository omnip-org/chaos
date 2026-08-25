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
| [`capability-map.md`](capability-map.md) | File-level navigation map for AI-assisted changes and cross-layer flows |
| [`database-conventions.md`](database-conventions.md) | Required schema, SQL, isolation, migration, money, and time rules |
| [`deployment.md`](deployment.md) | Production topology, secrets, bootstrap, rollout, and rollback |
| [`postgresql-extensions.md`](postgresql-extensions.md) | PostgreSQL image and extension lifecycle |
| [`adr/`](adr/) | Historical decisions and their status; an amended ADR must be read with its named successor |

## Runtime entry points

| Runtime | Entry point | Responsibility |
| --- | --- | --- |
| HTTP API | `crates/chaos-api/src/bin/chaos-api.rs` | Identity bootstrap, Storefront APIs, Provider webhooks, and health |
| MCP | `crates/chaos-api/src/mcp/router.rs` | AI-operated Store administration authenticated by User Access Keys |
| Worker | `crates/chaos-worker/src/bin/chaos-worker.rs` | Durable polling and Provider reconciliation outside API replicas |
| Migration job | `crates/chaos-api/src/bin/chaos-migrate.rs` | Applies SQL migrations before an application rollout |

## Dependency layers

| Crate | Look here for | Must not own |
| --- | --- | --- |
| `chaos-domain` | Business types, validation, and state transitions | HTTP, SQL, serialization, Provider SDKs |
| `chaos-core` | Use cases, PostgreSQL repositories, runtime, security, storage, and external Provider adapters | Axum handlers, transport DTOs, and API routing |
| `chaos-api` | HTTP routes, MCP tools, DTOs, runtime composition, and API delivery | Direct business persistence |
| `chaos-worker` | Worker composition and durable polling loops | HTTP routes and MCP transport |

## Change routing

| Product area | Domain and use cases | Delivery and adapters | Database ownership |
| --- | --- | --- | --- |
| Users, external identity, Access Keys | `identity` | HTTP auth and identity adapter | `identity` |
| Stores, memberships, channels, Publishable Keys | `store` | MCP Store tools and Store repositories | `commerce` |
| Products, variants, collections, media | `catalog` | MCP catalog tools and catalog repositories | `commerce` |
| Price lists | `pricing` | MCP pricing tools and pricing repositories | `commerce` |
| Stock and reservations | `inventory` | MCP inventory tools and inventory repository | `commerce` |
| Shopper, carts, checkout, and orders | `sales` | Storefront HTTP and sales repositories | `commerce` |
| Payments and refunds | `payments` | MCP payment tools and Stripe adapters | `commerce` |
| Shipping, fulfillment, and returns | `fulfillment` | MCP fulfillment tools and shipping adapters | `commerce` |
| Payment webhooks and payment queues | core payment workflows | Stripe adapters and Worker loops | `commerce` |
| Generic outbox and event routing | core event workflows | Worker loops and integration repositories | `integration` |
| Commerce events and external provider delivery | `analytics` | Storefront collection, MCP settings, and Worker delivery | `integration` |

Rust business modules remain useful navigation boundaries; they do not require
matching PostgreSQL schemas.

HTTP delivery code is grouped by public responsibility under
`crates/chaos-api/src/http/`:

- `identity/` contains account bootstrap and User Access Key endpoints;
- `storefront/` contains every publishable Store API surface;
- `storefront/v1/carts.rs` contains payment creation; Provider callbacks are mounted under `/integrations/v1/webhooks`;
- `health.rs` contains health checks;
- `shared/` contains transport extractors, envelopes, and test support.

MCP delivery keeps protocol concerns separate from commerce capabilities under
`crates/chaos-api/src/mcp/`:

- `router.rs`, `auth.rs`, `error.rs`, and `mutation.rs` own MCP transport,
  authentication, error mapping, and confirmation behavior;
- `tools/mod.rs` owns shared MCP state and is the single tool-router assembly
  point;
- `tools/store/`, `catalog/`, `pricing/`, `operations/`, and `integrations/`
  group tool implementations by product capability.

Moving a tool between capability modules must not rename the public tool or
change its input schema. Protocol-version changes belong in the MCP boundary,
not in application use cases.

## Contracts and operations

- `packages/js/` is the typed Storefront JavaScript client and public HTTP contract.
- `migrations/0001_platform.sql` through `0007_integration_analytics.sql`
  are the fresh bootstrap schema. `0004_integration.sql` owns the shared
  Integration account, webhook inbox, and queue infrastructure; the commerce
  schema
  schema persists one `commerce.shoppers` identity per website visit; cart,
  checkout, order, and analytics records follow that `shopper_id`.
- `deploy/` contains the production-equivalent Compose topology and origin TLS
  certificate used behind Cloudflare.
- `scripts/storefront-demo.mjs` exercises the supported commerce flow.

## Required verification

Run the commands listed in the root `README.md` and `CONTRIBUTING.md`. Database
changes additionally require a disposable PostgreSQL migration run and Store
isolation tests.
