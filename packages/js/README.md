# @omnip-org/chaos-js

A typed client and first-party analytics collector for the Chaos Commerce
[Store API](../../openapi/store-v1.json) — the public-key-authenticated
surface meant to be called directly from storefront browsers. One SDK covers
catalog browsing, cart, checkout, order, payment, and behavior analytics flows.

## Install

This package is published to GitHub Packages, not the public npm registry.
Add a `.npmrc` to the consuming project:

```
@omnip-org:registry=https://npm.pkg.github.com
```

Authenticate with a GitHub PAT that has `read:packages` scope (see
[docs/deployment.md](../../docs/deployment.md) for the equivalent GHCR PAT
setup used elsewhere in this repo), then:

```sh
npm install @omnip-org/chaos-js
```

## Usage

```ts
import { createStorefrontClient } from "@omnip-org/chaos-js";

const chaos = createStorefrontClient({
  publishableKey: "public_...",
  analytics: {
    providers: {
      // Must match the Store's Meta CAPI Dataset ID.
      metaPixel: { pixelId: "1234567890" },
      ga4: { measurementId: "G-EXAMPLE123" },
    },
  },
});

// Catalog
const { data: products } = await chaos.catalog.listProducts({ q: "shoes" });
const { data: product } = await chaos.catalog.getProduct("running-shoes");

// Cart — the shopper token is acquired and persisted automatically on the
// first mutating call, then reused for every subsequent Cart/Checkout call.
const { data: cart } = await chaos.cart.create();
await chaos.cart.addLine(cart.id, product.variants[0].id);

// Stripe Embedded Checkout — Chaos reserves inventory and creates the
// provisional Checkout/Order before Stripe collects the remaining details.
// The return URL must be HTTPS outside local loopback development.
const { data: session } = await chaos.payments.createEmbeddedCheckout(cart.id, {
  email: "shopper@example.com",
  return_url: "https://shop.example.com/checkout/success",
});
const { data: action } = await chaos.payments.getClientAction(session.payment_attempt_id);
// Pass action.client_token to Stripe's EmbeddedCheckoutProvider and initialize
// Stripe with loadStripe(action.public_key). Direct Stripe accounts do not use
// a Stripe-Account header or an account_reference field.

// PageView, ViewContent, Search, and active ViewDuration are recorded by the
// browser SDK. AddToCart, InitiateCheckout, AddPaymentInfo, Purchase, and
// Refund are recorded by the authoritative server workflows. After the server
// confirms payment, project Purchase with authoritative Order data:
chaos.analytics?.purchase({
  orderId: order.id,
  valueMinor: order.total_amount_minor,
  currency: order.currency,
  items: order.lines.map((line) => ({
    itemId: line.product_variant_id,
    quantity: line.quantity,
    priceMinor: line.unit_price_amount_minor,
  })),
});
// The server remains the source of truth for ledger Purchase and Refund events.
```

Pass `analytics: false` to `createStorefrontClient` to skip constructing the
collector entirely. Collection starts immediately when the analytics client is
constructed.

The client automatically acquires and persists the signed shopper token used to
associate commerce operations and Analytics events. The collector automatically
captures bounded UTM fields and the Referrer host.
It keeps first-touch, browser-session, and last-non-direct source facts.
Unsent events survive reloads in session storage, retain stable IDs during
retry, and drain in bounded batches. View duration uses a monotonic clock and
resumes correctly after browser back-forward cache restoration. Store-defined
behaviors can be recorded with `chaos.analytics?.track("wishlist_added", {
product_id: "..." })`.

Provider scripts are optional and load immediately when configured. Meta Pixel
receives the same event IDs used by CAPI. A confirmed Purchase uses the Order
ID in both paths and is projected only once per browser, allowing Meta to
deduplicate Pixel and CAPI copies. GA4 automatic PageView collection is
disabled; Chaos maps semantic events to GA4 ecommerce names.

### Server-side / SSR usage

`createStorefrontClient` defaults to same-origin relative URLs (`/store/v1/...`),
which relies on a browser `fetch` and `location`. From Node, an edge
function, or any non-browser environment, pass an absolute `baseUrl`:

```ts
const chaos = createStorefrontClient({
  publishableKey: process.env.CHAOS_PUBLISHABLE_KEY!,
  baseUrl: "https://shop.example.com/store/v1",
});
```

If a storefront receives an order tracking capability through its own notification
channel, it can exchange the capability and refresh the order without exposing it in a
URL:

```ts
const session = await chaos.orders.exchangeTrackingKey(trackingKey);
const tracked = await chaos.orders.getTrackedOrder(session.access_token);
console.log(tracked.order_number, tracked.delivery_status);
```

### Errors

Non-2xx responses reject with `ChaosApiError` (`status`, `code`, `message`,
`details`), matching the Store API's `{ error: { code, message, details? } }`
envelope.

```ts
import { ChaosApiError } from "@omnip-org/chaos-js";

try {
  await chaos.cart.setLine(cart.id, variantId, { quantity: 0 });
} catch (error) {
  if (error instanceof ChaosApiError && error.status === 422) {
    console.error(error.details); // [{ field: "quantity", reason: "..." }]
  }
}
```

## Development

```sh
npm run build --prefix packages/js
npm test --prefix packages/js
```

Types in `src/types.ts` are hand-written to mirror
[`openapi/store-v1.json`](../../openapi/store-v1.json); keep both in sync
when the Store API contract changes.
