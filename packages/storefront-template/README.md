# Storefront Template

A reference Astro + React + Tailwind storefront built on
[`@omnip-org/chaos-js`](../js), checking out through Stripe's hosted
Checkout page (the `stripe_checkout` provider — see
[ADR 0024](../../docs/adr/0024-stripe-checkout-session-support.md)).

This is a template to copy and adapt, not a package meant to be installed —
it's marked `"private": true` and not published.

## What it demonstrates

- Server-rendered (SSR) product listing and detail pages using
  `chaos.catalog.listProducts()` / `chaos.catalog.getProduct()`.
- A client-side cart (React island) persisting the Cart id in
  `localStorage` and calling `chaos.cart.create()` / `setLine()` /
  `removeLine()`.
- A full checkout flow: `chaos.checkout.create()` → `chaos.checkout.createOrder()`
  → `chaos.payments.createAttempt(orderId, { provider: "stripe_checkout", success_url, cancel_url })`
  → `chaos.payments.getClientAction(attemptId)` → redirect the browser to
  the returned Checkout Session URL.
- `success`/`cancel` pages Stripe redirects back to.

## Setup

```sh
cp .env.example .env
# fill in PUBLIC_CHAOS_PUBLISHABLE_KEY and PUBLIC_CHAOS_STORE_API_BASE_URL
npm install
npm run dev
```

Both environment variables are required, and both need the `PUBLIC_`
prefix — Astro/Vite only includes `PUBLIC_`-prefixed variables in the
browser bundle, and this template's cart/checkout React islands read them
client-side.

`PUBLIC_CHAOS_STORE_API_BASE_URL` must be an absolute URL (e.g.
`https://shop.example.com/store/v1`), even for local development. This
template renders its product pages server-side, and Node's `fetch` — unlike
a browser's — cannot resolve a relative URL against a page origin it
doesn't have.

## Server-side vs. browser-side client

`src/lib/chaos.ts` exports one `createChaosClient()` factory used
everywhere, but it behaves slightly differently depending on where it
runs (checked via Astro's `import.meta.env.SSR`):

- **Server (SSR pages)**: constructed with `analytics: false`, since the
  bundled analytics collector's constructor requires `document`/`window`,
  which don't exist under Node. Shopper-token persistence also falls back
  to none (no `localStorage` on the server) — fine, since SSR pages only
  do read-only catalog queries.
- **Browser (React islands)**: default configuration — analytics enabled,
  shopper token persisted in `localStorage` across requests.

## Deployment

`astro.config.mjs` uses the Node adapter in `standalone` mode, so
`npm run build` followed by `node ./dist/server/entry.mjs` runs the app
directly — no separate hosting-specific adapter needed. Deploy it anywhere
that can run a long-lived Node 22 process (Docker, a VM, or a Node-compatible
PaaS), with `PUBLIC_CHAOS_PUBLISHABLE_KEY` and
`PUBLIC_CHAOS_STORE_API_BASE_URL` set in the environment.

## What this template intentionally doesn't do

- **Multi-domain/multi-Store resolution.** The Store is determined
  entirely by which publishable key you configure. This template assumes
  each Store deploys its own storefront instance with its own key.
- **Order status on the success page.** Stripe's `success_url` only gets
  Stripe's own `session_id` back (via the `{CHECKOUT_SESSION_ID}`
  placeholder), not the Chaos `order_id`; the success page shows a static
  confirmation rather than fetching live order status.
