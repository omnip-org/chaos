# Store catalog provisioning

Applies `catalog.json` to one Chaos Store over MCP: a Product with generated
variants (cartesian product of the declared options), a Collection, and an
active Price List. Run it once per Store — once against a test Store, once
against a live Store — so both start from the same catalog definition instead
of being built by hand twice.

This script is unverified against a live Chaos deployment: it was written and
reviewed against the MCP tool signatures in `crates/chaos-mcp/src/tools/`, but
this environment had no running Chaos instance to exercise it against. Dry-run
it against a disposable test Store before trusting it against production.

## Prerequisites

```bash
cd scripts/provision-store
npm install
```

You need, per target Store:

1. **A Merchant Account and Store already created** (`POST /admin/v1/merchant-accounts`, `POST /admin/v1/merchant-accounts/{id}/stores` — requires a human bearer session; there is no machine-scriptable path to create a Store itself, since a Secret key can only exist *after* the Store it belongs to).
2. **`CHAOS_SALES_CHANNEL_ID`** — the Store's default Web Sales Channel UUID. No MCP tool lists Sales Channels; read it once from `GET /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/sales-channels` with a human bearer session.
3. **`CHAOS_SECRET_KEY`** — a Secret API key for that Store (`POST /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/api-keys`, human bearer session, `class: "secret"`) with scopes `mcp:tools`, `products:read`, `products:write`, `collections:write`, `pricing:write`. Save the plaintext immediately — it is only returned once.

## Run

```bash
CHAOS_BASE_URL=https://api.example.com \
CHAOS_SECRET_KEY=sk_... \
CHAOS_SALES_CHANNEL_ID=00000000-0000-0000-0000-000000000000 \
npm run provision
```

Point `CATALOG_FILE` at a different JSON file to provision a different catalog; otherwise it uses `catalog.json` next to this script.

## What this does not cover

No MCP tool exists yet for these — configure each once per Store through the
Admin API (human bearer session) or Admin UI before Checkout will work:

- **Shipping Service** — `POST /admin/v1/merchant-accounts/{id}/stores/{id}/shipping-services` (see `docs/http-api.md#storefront-carts-and-checkout`).
- **Tax Rule** — `POST /admin/v1/merchant-accounts/{id}/stores/{id}/tax-rules`, one active rule per destination country; Checkout creation fails without one.
- **Payment Provider Account** — Stripe Connect, per `docs/adr/0013-stripe-connect-direct-charges.md`. Requires a connected Stripe account meeting the direct-charge readiness checks before it can be enabled for live traffic.
- **Inventory** — variants are created with `track_inventory: true` but zero stock. Add an inventory location and adjust stock via the Admin API (`docs/http-api.md#inventory`) before the product can actually be purchased.

## Re-running

Every write uses a stable idempotency key derived from the relevant slice of
`catalog.json`, so re-running with an unchanged file is a safe no-op replay.
If you change `catalog.json` after a Store has already been provisioned, the
affected call fails with `idempotency_key_reused` instead of silently
applying the new content — this script bootstraps a fresh Store, it does not
reconcile an already-provisioned one with a changed definition. Use the Admin
API directly for catalog updates after first provisioning.
