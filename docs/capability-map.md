# Capability Map

This is the shortest path from a product capability to every layer that may
participate in it. Start with the capability row, then follow the flow from
delivery to application service, port, infrastructure adapter, and worker.

## Layer order

```text
HTTP or MCP delivery
        |
Application use case
        |
Application port  <---- Domain rules and value objects
        |
Infrastructure adapter
        |
PostgreSQL, Redis, object storage, or external Provider
```

`chaos-api` is the composition root. It constructs Application services with
Infrastructure implementations and passes the relevant services into HTTP and
MCP delivery. `chaos-mcp` must not access Infrastructure directly.

## Capability index

| Capability | HTTP delivery | MCP tools | Application | Ports | Infrastructure | Worker / queue |
| --- | --- | --- | --- | --- | --- | --- |
| Identity and Access Keys | `crates/chaos-api/src/http/identity/` | — | `crates/chaos-application/src/identity.rs` | `crates/chaos-application/src/ports/identity.rs` | `crates/chaos-infrastructure/src/security/identity.rs` | — |
| Stores and memberships | — | `crates/chaos-mcp/src/tools/store/` | `crates/chaos-application/src/store/` | `crates/chaos-application/src/ports/store*.rs` | `crates/chaos-infrastructure/src/repositories/store/` | — |
| Catalog and media | `crates/chaos-api/src/http/storefront/catalog.rs`, `collections.rs`, `reviews.rs` | `crates/chaos-mcp/src/tools/catalog/` | `crates/chaos-application/src/catalog/` | `crates/chaos-application/src/ports/catalog*.rs`, `collection.rs`, `media.rs`, `review.rs` | `crates/chaos-infrastructure/src/repositories/catalog/`, `storage/media.rs` | Search indexing in `crates/chaos-api/src/workers.rs` |
| Pricing, promotions, and tax | — | `crates/chaos-mcp/src/tools/pricing/` | `crates/chaos-application/src/pricing/` | `crates/chaos-application/src/ports/pricing*.rs`, `promotion.rs`, `tax.rs` | `crates/chaos-infrastructure/src/repositories/pricing/` | — |
| Storefront catalog | `crates/chaos-api/src/http/storefront/catalog.rs`, `collections.rs` | — | `crates/chaos-application/src/storefront.rs` | `crates/chaos-application/src/ports/storefront_catalog.rs` | `crates/chaos-infrastructure/src/repositories/sales/storefront_catalog.rs` | Search indexer in `crates/chaos-infrastructure/src/repositories/search/` |
| Shopper, cart, checkout, and order | `crates/chaos-api/src/http/storefront/sales.rs` | `crates/chaos-mcp/src/tools/operations/orders.rs` | `crates/chaos-application/src/sales/` | `crates/chaos-application/src/ports/sales.rs` | `crates/chaos-infrastructure/src/repositories/sales/` | Checkout expiry in `crates/chaos-api/src/workers.rs` |
| Inventory and reservations | — | `crates/chaos-mcp/src/tools/operations/inventory.rs` | `crates/chaos-application/src/inventory/` | `crates/chaos-application/src/ports/inventory.rs` | `crates/chaos-infrastructure/src/repositories/inventory/` | Reservation transitions are called by sales and payment workflows |
| Payments and refunds | `crates/chaos-api/src/http/storefront/payments.rs` | `crates/chaos-mcp/src/tools/operations/payments.rs` | `crates/chaos-application/src/payments/` | `crates/chaos-application/src/ports/payments.rs` | `crates/chaos-infrastructure/src/repositories/payments/`, `integrations/payments/stripe.rs` | Payment command and readiness workers in `crates/chaos-api/src/workers.rs` |
| Fulfillment and returns | — | `crates/chaos-mcp/src/tools/operations/fulfillment.rs` | `crates/chaos-application/src/fulfillment/` | `crates/chaos-application/src/ports/fulfillment.rs` | `crates/chaos-infrastructure/src/repositories/fulfillment/`, `integrations/shipping/easypost.rs` | Fulfillment event worker in `crates/chaos-api/src/workers.rs` |
| Analytics and Meta delivery | `crates/chaos-api/src/http/storefront/analytics.rs` | `crates/chaos-mcp/src/tools/integrations/analytics.rs` | `crates/chaos-application/src/analytics.rs` | `crates/chaos-application/src/ports/analytics.rs` | `crates/chaos-infrastructure/src/repositories/analytics/`, `integrations/analytics/` | Analytics delivery worker in `crates/chaos-api/src/runtime.rs` and `workers.rs` |
| Provider secrets | — | `crates/chaos-mcp/src/tools/integrations/provider_secrets.rs` | `crates/chaos-application/src/store/provider_secrets.rs` | `crates/chaos-application/src/ports/provider_secret.rs` | `crates/chaos-infrastructure/src/security/provider_secrets.rs` and Store repositories | — |

## Important cross-capability flows

### Product publication

```text
MCP catalog/products.rs
  -> application/catalog/create_product.rs or management.rs
  -> catalog provisioning / management ports
  -> repositories/catalog/catalog_*.rs
  -> commerce catalog tables
```

Storefront visibility continues through:

```text
HTTP storefront/catalog.rs
  -> application/storefront.rs
  -> ports/storefront_catalog.rs
  -> repositories/sales/storefront_catalog.rs
  -> published catalog + active price + media data
```

### Checkout and payment

```text
HTTP storefront/sales.rs
  -> application/sales/
  -> ports/sales.rs
  -> repositories/sales/storefront_sales/
  -> cart / checkout / order snapshot + inventory reservation

HTTP storefront/payments.rs
  -> application/payments/
  -> ports/payments.rs
  -> repositories/payments/
  -> integrations/payments/stripe.rs
  -> payment worker / order settlement / reservation closure
```

When changing checkout or payment, inspect both `sales` and `payments`, plus
inventory reservation helpers and the corresponding Worker entry point.

### Analytics and Meta

```text
HTTP storefront/analytics.rs
  -> application/analytics.rs
  -> ports/analytics.rs
  -> repositories/analytics/
  -> integrations/analytics/meta.rs
  -> analytics delivery worker
```

The stored event ledger is authoritative. Meta delivery is a retryable
projection and must not replace the internal event write.

## Registration and composition points

These files are intentionally high-signal entry points and must be checked
when adding a route, tool, service, or worker:

| Concern | Registration point |
| --- | --- |
| HTTP module tree | `crates/chaos-api/src/http/mod.rs` and `http/*/mod.rs` |
| HTTP route assembly | `crates/chaos-api/src/http/mod.rs::router` |
| MCP module tree | `crates/chaos-mcp/src/tools/*/mod.rs` |
| MCP tool assembly | `crates/chaos-mcp/src/tools/mod.rs::ChaosMcp::tool_router` |
| Application service construction | `crates/chaos-api/src/http/mod.rs::ApiState::new` |
| Worker dependency construction | `crates/chaos-api/src/runtime.rs` |
| Worker polling and dispatch | `crates/chaos-api/src/workers.rs` |
| Repository public exports | `crates/chaos-infrastructure/src/repositories/mod.rs` |
| Database ownership | `migrations/0001_platform.sql` through `0004_integration.sql` |

If a new file is added but one of these registration points is not updated,
the code may compile while the route, MCP tool, or Worker remains unreachable.

## Change checklist

For a capability change, search and verify in this order:

1. Find the delivery entry point in the capability index.
2. Find the Application service and its port.
3. Find the Infrastructure implementation of that port.
4. Follow any referenced queue, outbox, integration, or Worker.
5. Check the registration point listed above.
6. Check the relevant migration, OpenAPI contract, MCP tool description, and tests.

Use the capability name and the domain term together when searching. For
example, checkout work should search for `checkout`, `reservation`, and
`payment`, not only `sales`.
