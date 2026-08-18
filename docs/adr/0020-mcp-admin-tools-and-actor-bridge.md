# ADR 0020: MCP Admin Tools and the Actor Bridge

- Status: Accepted
- Date: 2026-08-16

## Context

ADR 0004 committed Chaos Commerce to exposing an MCP surface authenticated by the same Store-scoped machine credentials as the Store API, with `mcp:tools` plus a tool-specific scope required on every call. It left the transport and the first tool batch unimplemented.

Every existing admin write and read use case (catalog management, pricing management, catalog reads) is hard-typed on `actor: MerchantActor`, a struct only constructible from a real human passwordless session (`MerchantActor::new` is `pub(crate)`, built only inside `MerchantQueries::authorize`). The API-key-authenticated `MachineActor` MCP calls produce has no `user_id` or `MerchantRole` and had no path into any admin use case before this change.

## Decision

### Actor bridge

`AdminActor` (`crates/chaos-application/src/ports/actor.rs`) is a closed enum, `Merchant(MerchantActor) | Machine(MachineActor)`, used at admin port boundaries (`CatalogManagementUnitOfWork::begin`, `PricingManagementUnitOfWork::begin`, `PricingProvisioningUnitOfWork::begin`, `CatalogReadRepository`, `PricingReadRepository`, `InventoryRepository::list_stock`, `OrderManagementRepository::{list_orders,get_order}`) in place of `MerchantActor`. It exposes `merchant_account_id()` and `audit_user_id() -> Option<UserId>` (`Some` for a human, `None` for a machine). `MerchantActor` itself is untouched — no new fields, no visibility change, identical role-based authorization for the human path.

Infra unit-of-work implementations branch on `audit_user_id()`: a human sets PostgreSQL's `app.user_id` session variable as before; a machine explicitly clears it (`set_config('app.user_id', '', true)`) rather than leaving a stale value from a pooled connection. This is safe because `catalog.products` and `pricing.price_lists` carry no `created_by_user_id` audit column, and their RLS policies key only on `app.merchant_account_id` — `app.user_id` was never load-bearing for these tables' isolation, only for the `'user'`-scope branch of `idempotency_scope_isolation` and the merchant-account-directory policies, neither of which this call path exercises (it uses `IdempotencyScope::MerchantAccount`).

### Machine write authorization

A machine actor's scopes ARE its authorization — there is no attempt to map a `MachineActor` onto a `MerchantRole`. Every `require_*_writer` helper gained a `Machine` arm that checks scope presence (e.g. `ApiKeyScope::ProductsWrite`), alongside the unchanged `Merchant` arm's role match. This is checked twice per write: once at the MCP tool boundary (`authenticate_machine` requires `[McpTools, <scope>]`) and again inside the use case itself, so no use case ever trusts its caller implicitly.

### New scopes

`ApiKeyScope` (`crates/chaos-domain/src/merchant/api_key.rs`) gained `ProductsRead`, `ProductsWrite`, `PricingRead`, `PricingWrite`, `InventoryRead` — all excluded from `allowed_for_publishable_key()`, Secret-key only. They are not aliases for the existing storefront-facing `CatalogRead`: admin reads expose draft/archived data a publishable (browser-embeddable) key must never see. `OrdersRead` (pre-existing) is reused as-is. `ApiKeyScope` is a Postgres native enum (`merchant.api_key_scope`), defined in `migrations/0002_create_identity_schema.sql`.

### Transport and crate placement

MCP lives in a new crate, `crates/chaos-mcp`, sibling to `chaos-api` rather than a module inside it — consistent with the workspace's existing "new crate per architectural adapter" convention, and keeping ADR 0004's "MCP transport concerns stay outside domain and application layers" boundary from blurring into `chaos-api`'s REST-handler-shaped code. It depends on `chaos-application`/`chaos-domain` only; `chaos-api` depends on it and mounts its router.

Built on the official `rmcp` SDK (`rmcp = "3.1.2"`, features `server`, `transport-streamable-http-server`, `transport-streamable-http-server-session`). Tools are declared with `#[tool_router(router = ..., vis = "pub(super)")]` blocks split one per resource module (`tools/products.rs`, `tools/price_lists.rs`, `tools/inventory.rs`, `tools/orders.rs`), combined in `tools/mod.rs` via `router_a() + router_b() + ...` — the SDK's documented pattern for splitting tools across files while keeping them all on one `ServerHandler` type. Sessions are held in-process (`LocalSessionManager`) since every tool call re-authenticates independently from its own `Authorization` header rather than relying on session-bound identity; nothing survives a restart that a client can't recover by re-sending the header. `StreamableHttpService` implements `tower_service::Service`, mounted via `Router::fallback_service` (axum 0.8 forbids `nest_service("/", ...)` at the root) at `/mcp/v1` on the same Axum router and TCP listener `chaos-api` already serves `/admin/v1` and `/store/v1` from — no second port or process.

Each tool authenticates independently per call: `authenticate_machine` (`crates/chaos-mcp/src/auth.rs`) reads the `Authorization` header via `rmcp::handler::server::common::Extension<http::request::Parts>` (an rmcp-provided extractor, distinct from `axum::Extension`) and calls the same `ApiKeyAuthentication::authenticate(&token, &[McpTools, <scope>])` the Store API extractors use.

### Error mapping

`ApplicationError` maps to `CallToolResult::structured_error` (`crates/chaos-mcp/src/error.rs`), not `Err(ErrorData)`, for every outcome the caller should be able to read and act on — wrong scope, validation failure, not found, conflict. rmcp's own documentation is explicit that MCP clients typically render `Err(ErrorData)` opaquely but always render `CallToolResult` content, so reserving protocol errors for genuinely unroutable requests (there are none in this batch) keeps every failure legible to the calling agent.

### Confirmation and idempotency

Every write tool's input schema requires two fields with no default: `confirm: bool` and `idempotency_key: String`. `require_confirmation` (`crates/chaos-mcp/src/mutation.rs`) rejects `confirm != true` before any use-case call — stateless, no server-side pending-operation store, matching ADR 0004's "explicit confirmation semantics" with the minimum viable mechanism. The server derives `IdempotencyRequest.request_fingerprint` itself (SHA-256 of the full serialized tool input) rather than trusting a client-supplied value, reusing the existing `IdempotencyRequest { key, request_fingerprint }` shape unchanged; `idempotency_key` supplies only the `key`, matching how the REST admin API's `Idempotency-Key` header works today.

## First tool batch

Read: `list_products`, `get_product`, `list_price_lists`, `get_price_list`, `list_inventory`, `list_orders`, `get_order`.
Write: `update_product`, `activate_product`, `archive_product`, `publish_product`, `unpublish_product`, `create_price_list`, `update_price_list`, `activate_price_list`, `archive_price_list`.

Order status transitions, inventory location/stock writes, and promotions/tax-rule management are out of scope for this batch — `AdminActor` is not yet threaded through those ports.

## Consequences

- Admin sessions (human) and MCP/API-key access remain on separate, non-interchangeable actor types; `AdminActor` only unifies them at the narrow set of ports that now accept either.
- A future MCP tool batch (customers, orders-write, promotions/tax rules) follows the same pattern: extend the port signature to `AdminActor`, add a `Machine` arm to its writer-check, add the scope, add the tool.
- `chaos-mcp` evolves independently of `chaos-api`'s REST contract even where both call the same use cases, per ADR 0004.
- Losing an API key's `mcp:tools` or a specific write scope immediately removes that capability from every session using it — nothing is cached beyond the existing key-lookup path.

## Rejected alternatives

### Mapping machine scopes onto `MerchantRole`

Synthesizing a role for a non-human caller would be a fiction with no real membership behind it. Scope presence is what the API key issuer (a role-authorized human, per ADR 0004) actually delegated; treating scopes as the authorization directly is more honest than round-tripping through a role that was never granted.

### Stateful propose/commit confirmation flow

A two-step flow needs server-side state for the pending operation between calls — its own storage (likely Redis), expiry/GC, and idempotency-within-idempotency. A required `confirm: true` field is stateless and is the minimum mechanism that satisfies "explicit confirmation semantics."
