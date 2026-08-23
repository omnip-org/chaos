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
| HTTP API | `crates/chaos-api/src/main.rs` | Identity bootstrap, Storefront APIs, Provider webhooks, and health |
| MCP | `crates/chaos-api/src/mcp/router.rs` | AI-operated Store administration authenticated by User Access Keys |
| Worker | `crates/chaos-worker/src/main.rs` | Durable polling and Provider reconciliation outside API replicas |
| Migration job | `crates/chaos-api/src/bin/chaos-migrate.rs` | Applies SQL migrations before an application rollout |

## Dependency layers

| Crate | Look here for | Must not own |
| --- | --- | --- |
| `chaos-domain` | Business types, validation, and state transitions | HTTP, SQL, serialization, Provider SDKs |
| `chaos-application` | Use cases and ports | Axum handlers and SQL queries |
| `chaos-infrastructure` | PostgreSQL repositories and external Provider adapters | Transport DTOs and new business rules |
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
| Payment webhooks and payment queues | application ports | Stripe adapters and Worker loops | `commerce` |
| Generic outbox, event routing, and idempotency | application ports | Worker loops and integration repositories | `integration` |
| Commerce events and external provider delivery | `analytics` | Storefront collection, MCP settings, and Worker delivery | `integration` |

Rust business modules remain useful navigation boundaries; they do not require
matching PostgreSQL schemas.

HTTP delivery code is grouped by public responsibility under
`crates/chaos-api/src/http/`:

- `identity/` contains account bootstrap and User Access Key endpoints;
- `storefront/` contains every publishable Store API surface;
- `storefront/payments.rs` contains payment creation and Provider callback endpoints;
- `operations/` contains health checks;
- `shared/` contains transport extractors, envelopes, OpenAPI, and test support.

MCP delivery keeps protocol concerns separate from commerce capabilities under
`crates/chaos-api/src/mcp/`:

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
- `migrations/0001_platform.sql`, `0002_identity.sql`,
  `0003_commerce.sql`, `0004_commerce_catalog.sql`,
  `0005_commerce_pricing.sql`, `0006_commerce_sales.sql`,
  `0007_integration.sql`, `0008_integration_analytics.sql`, and
  `0009_commerce_payments.sql` are the fresh bootstrap schema. The commerce
  schema persists one `commerce.shoppers` identity per website visit; cart,
  checkout, order, and analytics records follow that `shopper_id`.
- `deploy/` contains the production-equivalent Compose topology and origin TLS
  certificate used behind Cloudflare.
- `scripts/storefront-demo.mjs` exercises the supported commerce flow.

## Required verification

Run the commands listed in the root `README.md` and `CONTRIBUTING.md`. Database
changes additionally require a disposable PostgreSQL migration run and Store
isolation tests.
