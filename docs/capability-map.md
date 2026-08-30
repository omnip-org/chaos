# Capability Map

This is the shortest path from a product capability to every layer that may
participate in it. Start with the capability row, then follow the flow from
delivery to core use case, concrete repository or external seam, and worker.

## Layer order

```text
HTTP or MCP delivery
        |
Core use case <---- Domain rules and value objects
        |
Concrete repository or external seam
        |
PostgreSQL, Redis, object storage, or external Provider
```

`chaos-api` is the HTTP and MCP composition root. It constructs core services
with concrete PostgreSQL repositories and the few external adapters that need
replacement, then passes the relevant services into HTTP and MCP delivery.
`chaos-worker` is the separate background-worker composition root.

## Capability index

| Capability | HTTP delivery | MCP tools | Core use case | External seams | Repositories / adapters | Worker / queue |
| --- | --- | --- | --- | --- | --- | --- |
| Identity, MCP OAuth, and Access Keys | `crates/chaos-api/src/http/identity/v1/`, `crates/chaos-api/src/http/oauth.rs` | — | `crates/chaos-core/src/identity/` | `crates/chaos-core/src/contracts/identity.rs` | `crates/chaos-core/src/adapters/security/identity.rs`, `crates/chaos-core/src/adapters/security/mcp_oauth.rs` | — |
| Stores, memberships, and shipping countries | — | `crates/chaos-api/src/mcp/tools/store/` | `crates/chaos-core/src/store/` | `crates/chaos-core/src/contracts/store*.rs` | `crates/chaos-core/src/adapters/postgres/store/` | — |
| Catalog, media, and reviews | `crates/chaos-api/src/http/storefront/v1/products.rs`, `collections.rs` | `crates/chaos-api/src/mcp/tools/catalog/` | `crates/chaos-core/src/catalog/` | `crates/chaos-core/src/contracts/catalog*.rs`, `collection.rs`, `media.rs`, `review.rs` | `crates/chaos-core/src/adapters/postgres/catalog/`, `adapters/storage/media.rs` | Search indexing in `crates/chaos-core/src/adapters/postgres/search/` |
| Price lists | — | `crates/chaos-api/src/mcp/tools/pricing/` | `crates/chaos-core/src/pricing/` | `crates/chaos-core/src/contracts/pricing.rs` | `crates/chaos-core/src/adapters/postgres/pricing/` | — |
| Storefront catalog | `crates/chaos-api/src/http/storefront/v1/products.rs`, `collections.rs` | — | `crates/chaos-core/src/catalog/storefront.rs` | `crates/chaos-core/src/contracts/storefront_catalog.rs` | `crates/chaos-core/src/adapters/postgres/sales/storefront_catalog.rs` | Search indexer in `crates/chaos-core/src/adapters/postgres/search/`; Product media rules resolve exact Variant -> Option Value -> Product fallback |
| Shopper, cart, checkout, and order | `crates/chaos-api/src/http/storefront/v1/shopper.rs`, `carts.rs`, `orders.rs` | `crates/chaos-api/src/mcp/tools/operations/orders.rs` | `crates/chaos-core/src/sales/` | `crates/chaos-core/src/contracts/sales.rs` | `crates/chaos-core/src/adapters/postgres/sales/` | Checkout expiry in `crates/chaos-worker/src/workers.rs` |
| Inventory and reservations | — | `crates/chaos-api/src/mcp/tools/operations/inventory.rs` | `crates/chaos-core/src/inventory/` | `crates/chaos-core/src/contracts/inventory.rs` | `crates/chaos-core/src/adapters/postgres/inventory/` | Reservation transitions are called by sales and payment workflows |
| Payments and refunds | `crates/chaos-api/src/http/storefront/v1/carts.rs`, `crates/chaos-api/src/http/storefront/v1/orders.rs`, `crates/chaos-api/src/http/storefront/v1/webhooks.rs` | `crates/chaos-api/src/mcp/tools/operations/payments.rs` | `crates/chaos-core/src/payments/` | `crates/chaos-core/src/contracts/stripe.rs` (`PaymentProvider`, registry) | `crates/chaos-core/src/adapters/postgres/payments/`, `adapters/integrations/stripe.rs` | Checkout Session creation is synchronous; refund commands use `chaos_payment_commands`, and verified payment webhooks use `chaos_webhooks`; payment state remains in `commerce` |
| Email delivery | `crates/chaos-api/src/http/storefront/v1/webhooks.rs` mounted at `/integrations/v1` | `crates/chaos-api/src/mcp/tools/integrations/email.rs`, `provider_secrets.rs` | `crates/chaos-core/src/email.rs` | `crates/chaos-core/src/contracts/email.rs` | `crates/chaos-core/src/adapters/postgres/integrations/email.rs`, `adapters/integrations/resend.rs` | `order.confirmed` routes to `chaos_email_commands`; the global server-owned template renders order data and Store brand tokens live under the Email account's `integration.provider_accounts.configuration.brand`; verified email events use the shared `chaos_webhooks` inbox |
| Fulfillment (manual shipping) | — | `crates/chaos-api/src/mcp/tools/operations/fulfillment.rs` | `crates/chaos-core/src/fulfillment/` and `shipping.rs` | `crates/chaos-core/src/contracts/shipping.rs` | `crates/chaos-core/src/adapters/postgres/fulfillment/`, `adapters/integrations/manual_shipping.rs` | Fulfillment state remains in `commerce`; manual account lookup and Shipping dispatch use `integration` and `chaos_shipping_commands`; Returns and carrier integration are not implemented |
| Analytics and Meta delivery | `crates/chaos-api/src/http/storefront/v1/analytics.rs` | `crates/chaos-api/src/mcp/tools/integrations/analytics.rs` | `crates/chaos-core/src/analytics/` | `crates/chaos-core/src/contracts/analytics.rs` | `crates/chaos-core/src/adapters/postgres/analytics/`, `adapters/integrations/analytics/` | Analytics delivery worker in `crates/chaos-worker/src/workers.rs` |
| Provider secrets | — | `crates/chaos-api/src/mcp/tools/integrations/provider_secrets.rs` | `crates/chaos-core/src/store/provider_secrets.rs` | `crates/chaos-core/src/contracts/provider_secret.rs` | `crates/chaos-core/src/adapters/security/provider_secrets.rs` and Store repositories | — |

## Important cross-capability flows

### Product configuration and media editing

```text
MCP catalog/products.rs + media.rs
  -> chaos-core catalog/workspace.rs, configuration.rs, media.rs
  -> catalog read/configuration/media repositories
  -> commerce products, options, variants, and reusable media link tables
```

The Product revision is the optimistic-concurrency boundary for canonical
content, configuration, publication, and Product gallery mutations. Storefront
media remains raw rule data plus exact Variant -> Option Value -> Product
fallback resolution; the JavaScript SDK mirrors that resolver.

### Product publication

```text
MCP catalog/products.rs
  -> chaos-core catalog/create_product.rs or management.rs
  -> catalog provisioning / management contracts
  -> adapters/postgres/catalog/catalog_*.rs
  -> commerce catalog tables
```

Storefront visibility continues through:

```text
HTTP storefront/v1/products.rs + collections.rs
  -> chaos-core catalog/storefront.rs
  -> contracts/storefront_catalog.rs
  -> adapters/postgres/sales/storefront_catalog.rs
  -> published catalog + active price + media data
```

### Checkout and payment

```text
HTTP storefront/v1/shopper.rs + carts.rs + orders.rs
  -> chaos-core sales/
  -> contracts/sales.rs
  -> adapters/postgres/sales/
  -> cart / checkout / order snapshot + inventory reservation

HTTP storefront/v1/carts.rs + storefront/v1/webhooks.rs (mounted at /integrations/v1)
  -> chaos-core payments/
  -> contracts/stripe.rs (`PaymentProvider` port and registry)
  -> adapters/postgres/payments/
  -> adapters/integrations/stripe.rs
  -> integration.provider_webhook_inbox / payment worker / order settlement / reservation closure
```

When changing checkout or payment, inspect the cart/order and checkout/webhook
HTTP modules, plus inventory reservation helpers and the corresponding Worker
entry point.

### Analytics and Meta

```text
HTTP storefront/v1/analytics.rs
  -> chaos-core analytics/
  -> contracts/analytics.rs
  -> adapters/postgres/analytics/
  -> adapters/integrations/analytics/meta.rs
  -> analytics delivery worker
```

The stored event ledger is authoritative. Meta delivery is a retryable
projection and must not replace the internal event write.

### Provider webhook ingress

```text
HTTP provider route
  -> capability verifier (Stripe / Resend / future carrier)
  -> VerifiedWebhookEvent
  -> integration.provider_webhook_inbox unique inbox
  -> chaos_webhooks
  -> capability worker / Commerce state transition
```

Only the verifier knows the provider signature format. The Integration
repository owns account resolution, idempotency, queue enqueue, visibility
leases, retries, and terminal processing evidence. A verified event keeps its
raw `provider_event_type`; a known event also receives a
`normalized_event_type`, while an unknown event is retained and finished as
`unsupported`.

## Registration and composition points

These files are intentionally high-signal entry points and must be checked
when adding a route, tool, service, or worker:

| Concern | Registration point |
| --- | --- |
| HTTP module tree | `crates/chaos-api/src/http/mod.rs` and `http/*/mod.rs` |
| HTTP route assembly | `crates/chaos-api/src/http/mod.rs::router` |
| MCP module tree | `crates/chaos-api/src/mcp/tools/*/mod.rs` |
| MCP tool assembly | `crates/chaos-api/src/mcp/tools/mod.rs::ChaosMcp::tool_router` |
| Application service construction | `crates/chaos-api/src/http/mod.rs::ApiState::new` |
| Worker dependency construction | `crates/chaos-worker/src/runtime.rs` |
| Worker polling and dispatch | `crates/chaos-worker/src/workers.rs` |
| Repository public exports | `crates/chaos-core/src/adapters/postgres/mod.rs` |
| Database ownership | `migrations/0001_platform.sql` through `migrations/0012_publishable_key_channel_binding.sql`; Store, catalog, sales, payments, checkout lifecycle, and fulfillment business objects use `commerce`, while identity/OAuth state uses `identity`, and Provider accounts, webhook inboxes, and event routing use `integration` |

If a new file is added but one of these registration points is not updated,
the code may compile while the route, MCP tool, or Worker remains unreachable.

## Change checklist

For a capability change, search and verify in this order:

1. Find the delivery entry point in the capability index.
2. Find the core use case and its concrete repository or external seam.
3. Find the repository or adapter implementation in the same core module.
4. Follow any referenced queue, outbox, integration, or Worker.
5. Check the registration point listed above.
6. Check the relevant migration, SDK contract, MCP tool description, and tests.

Use the capability name and the domain term together when searching. For
example, checkout work should search for `checkout`, `reservation`, and
`payment`, not only `sales`.
