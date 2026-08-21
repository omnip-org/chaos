# @omnip-org/chaos-js

A typed client and first-party analytics collector for the Chaos Commerce
[Store API](../../openapi/store-v1.json) — the publishable-key-authenticated
surface meant to be called directly from storefront browsers. One SDK covers
catalog browsing, cart, checkout, order, customer, and payment flows, plus
the same consent-aware analytics collector previously shipped as the
standalone `storefront-analytics` package.

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
  publishableKey: "pk_live_...",
  analytics: {
    privacyMode: "opt_out",
    consent: {
      analyticsStorage: true,
      advertisingStorage: false,
      policyVersion: "cmp-2026-08",
    },
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

// Checkout and Order
const { data: checkout } = await chaos.checkout.create(cart.id, {
  contact: { email: "shopper@example.com" },
  billing_address: { full_name: "Ada Lovelace", address_line1: "1 Main St", locality: "London", country_code: "GB" },
});
const { data: order } = await chaos.checkout.createOrder(checkout.id);

// Stripe Embedded Checkout — amount and currency are taken from the immutable
// Chaos Order. The return URL must be HTTPS outside local loopback development.
const { data: attempt } = await chaos.payments.createAttempt(order.id, {
  provider: "stripe_checkout",
  return_url: "https://shop.example.com/checkout/success?order_id=" + order.id,
});
const { data: action } = await chaos.payments.getClientAction(attempt.id);
// Pass action.client_token to Stripe's EmbeddedCheckoutProvider and initialize
// Stripe with loadStripe(action.public_key). Direct Stripe accounts do not use
// a Stripe-Account header or an account_reference field.

// PageView, ViewContent, Search, AddToCart, InitiateCheckout, and active
// ViewDuration are recorded by the SDK operations above. After the server
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
collector entirely. The default `opt_out` mode starts collection immediately;
`opt_in` keeps it disabled until `setConsent()` grants analytics storage.

The collector automatically captures bounded UTM fields and the Referrer host.
It keeps first-touch, browser-session, and last-non-direct source facts;
advertising click IDs are included only with advertising-storage consent.
Unsent events survive reloads in session storage, retain stable IDs during
retry, and drain in bounded batches. View duration uses a monotonic clock and
resumes correctly after browser back-forward cache restoration.

Provider scripts are optional and load immediately in the default `opt_out`
mode. Meta Pixel
receives the same event IDs used by CAPI. A confirmed Purchase uses the Order
ID in both paths and is projected only once per browser, allowing Meta to
deduplicate Pixel and CAPI copies. GA4 automatic PageView collection is
disabled; Chaos maps semantic events to GA4 ecommerce names.
Default events declare `collection_basis: "store_policy"`, and the server
accepts them only when the Store has the matching `opt_out` Analytics setting.
Calling `setConsent()` with both storage choices disabled stops first-party,
Meta Pixel, and GA4 collection. Stores that require prior consent can select
`privacyMode: "opt_in"` instead.

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

### Authenticated customers

Cart/Checkout/Order calls only need the publishable key (plus the
auto-managed shopper token). Customer-account endpoints
(`chaos.customer.*`) additionally require a customer session obtained
through the user identity flow — set it once and every
customer call attaches it:

```ts
chaos.setCustomerSession(sessionToken);
const { data: customer } = await chaos.customer.get();
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
