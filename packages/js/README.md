# @omnip-org/chaos-js

A typed client and first-party analytics collector for the Chaos Commerce
Storefront API — the public-key-authenticated surface meant to be called
directly from storefront browsers. The SDK is the typed public contract. One SDK covers
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
  publishableKey: "pk_...",
  analytics: {
    providers: {
      // Browser Meta Pixel ID; the CAPI Dataset ID is configured server-side.
      metaPixel: { pixelId: "1234567890" },
      ga4: { measurementId: "G-EXAMPLE123" },
    },
  },
});

// Catalog
const { data: products } = await chaos.catalog.listProducts({ q: "shoes" });
const { data: product } = await chaos.catalog.getProduct("running-shoes");

// Cart — the shopper token is acquired and persisted automatically on the
// first shopper-scoped call, then reused for every subsequent Cart/Checkout call.
const { data: cart } = await chaos.cart.create();
await chaos.cart.addLine(cart.id, product.variants[0].id);
// If a persisted cart has already completed checkout, getOrCreate returns a
// fresh active cart without changing the shopper identity.
const { data: activeCart } = await chaos.cart.getOrCreate(cart.id);
// Cart mutations use the response version as an If-Match precondition. The
// SDK reads and sends it automatically; direct HTTP callers must send the
// current Cart version in the If-Match header.

// Stripe Embedded Checkout — Chaos reserves inventory and creates the
// provisional Checkout/Order before Stripe collects the remaining details.
// The return URL must be HTTPS outside local loopback development.
const { data: session } = await chaos.payments.createEmbeddedCheckout(cart.id, {
  email: "shopper@example.com",
  returnUrl: "https://shop.example.com/checkout/success",
});
// The SDK derives and sends a stable internal idempotency key from the current
// cart snapshot. Cart mutations automatically move the request to a new key;
// callers never need to construct or persist one.
const action = session.client_action;
// Pass action.client_token to Stripe's EmbeddedCheckoutProvider and initialize
// Stripe with loadStripe(action.public_key). Direct Stripe accounts do not use
// a Stripe-Account header or an account_reference field.

// PageView, ViewContent, Search, and active ViewDuration are recorded by the
// browser SDK. Cart and checkout resources record their commerce events only
// after the matching operation succeeds. The SDK owns canonical event fields,
// event IDs, attribution, and the /analytics/events transport. Purchase
// remains a payment-confirmation event on the server; the SDK only projects
// the confirmed order to browser providers.
// After the server confirms payment, project Purchase with Order data:
chaos.analytics?.purchase({
  orderId: order.id,
  valueMinor: order.total_amount_minor,
  currency: order.currency,
  items: order.lines.map((line) => ({
    productId: line.product_id,
    productVariantId: line.product_variant_id,
    quantity: line.quantity,
    priceMinor: line.unit_price_amount_minor,
  })),
});
// The server remains the source of truth for the ledger Purchase event.
```

Pass `analytics: false` to `createStorefrontClient` to skip constructing the
collector entirely. Collection starts immediately when the analytics client is
constructed.

The client automatically acquires and persists the signed shopper token used to
associate commerce operations and Analytics events on the first shopper-scoped
request. `cart.getActive()` reads only an active cart, while
`cart.getOrCreate()` explicitly recovers from a missing or completed cart.
Invalid shopper-token retries are opt-in because silently minting a replacement
can orphan a cart or hide an order. The collector automatically
captures bounded UTM fields and the Referrer host.
It keeps first-touch, browser-session, and last-non-direct source facts.
Unsent events survive reloads in session storage, retain stable IDs during
retry, and drain in bounded batches. View duration uses a monotonic clock and
resumes correctly after browser back-forward cache restoration. Store-defined
behaviors can be recorded with `chaos.analytics?.track("store_defined_event", {
product_id: "..." })`.
Commerce item properties use `product_id` and `product_variant_id`. Multi-item
events repeat these fields inside each `items[]` entry; single-item events also
expose the corresponding IDs at the top level. The Meta adapter uses the
variant ID as the Meta content ID when present, otherwise the product ID.
The SDK creates one commerce envelope containing the event ID, timestamp,
session, traffic, UTM values, and Meta browser context (`fbc`/`fbp`) after the
cart or checkout request succeeds. The business request does not contain an
analytics field. The SDK sends the envelope through `/analytics/events`; the
endpoint records it in the same ledger used by all browser observations. The
SDK projects the event ID to browser providers after success and persists the
first-party event for retry, so Meta can deduplicate the Pixel and CAPI copies.
The API normalizes the session UUID into the analytics event's nullable
`session_id` column and the UTM values into nullable `utm_*` columns. Use the
shopper ID and order/cart IDs as the durable association keys. First-touch and
last-non-direct traffic history remains in `properties`; UTM values are not
forwarded as Meta custom parameters.
Do not duplicate `add_to_cart`, `initiate_checkout`, or `purchase` through
generic `track()`. The SDK resources own the first two and send them through
the common endpoint after the matching request succeeds. `purchase` is
accepted only from payment confirmation; the SDK projects that confirmed
order with the Order ID, while the Meta CAPI adapter routes the server-origin
ledger event.

Provider scripts are optional and load immediately when configured. For Meta
events that are projected to both browser Pixel and CAPI, Pixel receives the
same event ID used by CAPI.
A confirmed Purchase uses the Order ID in both paths and is projected only once
per browser, allowing Meta to deduplicate Pixel and CAPI copies. View duration
and store-defined behavior events remain first-party ledger facts
and are not sent to Meta. PageView remains in the first-party ledger and may
be sent by the browser Pixel, but the server-side Meta CAPI adapter filters it
for now. The collector records the page URL without its
fragment, plus browser matching context (`fbc`, `fbp`, and user-agent),
alongside the event; when a
`fbclid` is present, the SDK also keeps the generated `_fbc` as a bounded
first-party cookie. The API may use matching request cookies as a fallback for
missing `fbc`/`fbp` values. For a server-side analytics bridge, pass its
inbound request as `request` when creating the client; the SDK copies the
edge-observed client IP into each analytics event and the API preserves the
event's client IP and user-agent metadata. GA4 automatic
PageView collection is disabled; Chaos maps semantic events to GA4 ecommerce
names.

### Server-side / SSR usage

`createStorefrontClient` defaults to same-origin relative URLs (`/storefront/v1/...`),
which relies on a browser `fetch` and `location`. From Node, an edge
function, or any non-browser environment, pass an absolute `baseUrl`:

```ts
const chaos = createStorefrontClient({
  publishableKey: process.env.CHAOS_PUBLISHABLE_KEY!,
  baseUrl: "https://shop.example.com/storefront/v1",
});
```

Confirmation emails link to the Sales Channel storefront's `/orders/track` page with the
tracking token in the URL fragment. The page reads the fragment locally and
submits it in the request body; the token is never placed in a query string:

```ts
const trackingToken = new URLSearchParams(window.location.hash.slice(1)).get(
  "token",
);
if (!trackingToken) throw new Error("missing order tracking token");
const tracked = await chaos.orders.getTrackedOrder(trackingToken);
console.log(tracked.order_number, tracked.shipping_status);
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

Types in `src/types.ts` are the hand-written Storefront wire contract; keep them
in sync with the public routes and response handlers when the Store API changes.
