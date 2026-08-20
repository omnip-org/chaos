# Storefront Template

A reference Astro + React + Tailwind storefront built on
[`@omnip-org/chaos-js`](../js). It demonstrates Chaos catalog, cart,
checkout, order, and Stripe Embedded Checkout in one small application.

This is a private template to copy and adapt, not a published package.

## End-to-end flow

1. A User signs in through the Identity HTTP API and creates a User Access Key.
2. The Access Key authenticates MCP calls that create a Store, product variants,
   prices, inventory, a storefront Publishable Key, and an optional Stripe
   payment account.
3. This storefront lists the published product and lets a shopper create a cart
   and order.
4. Chaos creates an embedded Stripe Checkout Session. The browser receives only
   its client secret and mounts Stripe's Embedded Checkout component.
5. A verified Stripe webhook confirms the payment. The return page polls the
   Chaos order until that server-side confirmation arrives.
6. Chaos emails a stable `/orders/track#...` link. The page removes the capability
   from browser history, exchanges it for a short-lived session, and displays the
   customer-facing order number and current status.
7. MCP creates a fulfillment, marks it shipped, and marks it delivered.

Payment success is never inferred from the browser return URL. The order changes
only after Chaos verifies and processes the Stripe webhook.

## Bootstrap the demo

Run Chaos API and the independent Worker first. Then provide either an existing
Chaos JWT or a Google/Apple identity token:

```sh
export CHAOS_API_ORIGIN=http://127.0.0.1:8080
export CHAOS_IDENTITY_PROVIDER=google
export CHAOS_IDENTITY_TOKEN='...'

# Optional: omit all three to bootstrap catalog/storefront without payments.
export STRIPE_SECRET_KEY='sk_test_...'
export STRIPE_PUBLISHABLE_KEY='pk_test_...'
export STRIPE_WEBHOOK_SECRET='whsec_...'

node scripts/storefront-demo.mjs setup
```

For an existing User JWT, set `CHAOS_USER_JWT` instead of the two identity
variables. For an existing User Access Key, set `CHAOS_ACCESS_KEY`; the script
then skips Identity HTTP calls. Stripe uses the account that owns the API key by
default, with a Store-unique `platform:` reference. Set
`STRIPE_ACCOUNT_REFERENCE=acct_...` only for Stripe Connect.

The script writes two ignored, mode-`0600` files:

- `packages/storefront-template/.env.demo` contains the browser-safe Store
  Publishable Key and Store API URL.
- `.env.storefront-demo` contains the User Access Key and Store id for subsequent
  MCP fulfillment calls. Keep it secret.

Start the template with Astro's `demo` environment mode:

```sh
cd packages/storefront-template
npm install
npm run dev -- --mode demo
```

`PUBLIC_CHAOS_STORE_API_BASE_URL` must be absolute, including locally, because
the server-rendered pages use Node `fetch`, which cannot resolve a relative URL.

The browser optionally loads GA4 and Meta Pixel through
`PUBLIC_GA4_MEASUREMENT_ID` and `PUBLIC_META_PIXEL_ID`. The Meta Pixel ID must
match the Dataset ID configured for CAPI so Chaos can reuse each stable event
ID for Provider deduplication. `PUBLIC_CHAOS_ANALYTICS_PRIVACY_MODE=opt_out` is
the default: Chaos, Meta Pixel, and GA4 start immediately when configured and
stop after an explicit shopper opt-out. Use `opt_in` only for a Storefront that
must wait for a prior choice.

## Stripe webhook

Configure Stripe to deliver Checkout events to:

```text
https://YOUR_CHAOS_ORIGIN/webhooks/v1/payments/stripe_checkout
```

The endpoint secret must be the value supplied as `STRIPE_WEBHOOK_SECRET` during
bootstrap. At minimum subscribe to `checkout.session.completed`,
`checkout.session.async_payment_succeeded`,
`checkout.session.async_payment_failed`, and `checkout.session.expired`.

For local testing, forward Stripe test events to the same path and use the
forwarder's `whsec_...` secret when running the bootstrap script.

## Fulfill the paid order through MCP

After Checkout returns and the order page shows payment confirmation, load the
ignored admin environment and run:

```sh
set -a
source .env.storefront-demo
set +a
node scripts/storefront-demo.mjs fulfill ORDER_UUID
```

The command reads the paid order lines through MCP, creates their fulfillment,
then transitions it through `shipped` and `delivered`. Override
`CHAOS_DEMO_CARRIER` and `CHAOS_DEMO_TRACKING_NUMBER` to use custom demo values.

## Server-side and browser-side clients

`src/lib/chaos.ts` exports one client factory. Server-side rendering disables
browser analytics and requires an absolute API URL. Browser islands enable the
normal analytics and shopper-token persistence behavior.

The Store is selected entirely by its Publishable Key. A multi-domain deployment
should resolve the correct key outside this template.

## Deployment

`astro.config.mjs` uses the Node adapter in standalone mode. Build with
`npm run build`, then run `node ./dist/server/entry.mjs` on Node 22 or newer.
