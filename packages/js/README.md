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
import {
  createStorefrontClient,
  resolveProductMedia,
  toPurchaseAnalyticsInput,
} from "@omnip-org/chaos-js";
import { mountEmbeddedCheckout } from "@omnip-org/chaos-js/stripe";

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

// Product media is returned as compact, reusable rules. Resolve the gallery
// after the shopper selects a Variant: exact Variant, matching Option Value,
// then Product fallback media.
const selectedVariant = product.variants[0]!;
const gallery = resolveProductMedia(product, selectedVariant);

// Cart — the shopper token is acquired and persisted automatically on the
// first shopper-scoped call, then reused for every subsequent Cart/Checkout call.
const { data: cart } = await chaos.cart.create();
await chaos.cart.addLine(cart.id, product.variants[0].id);
// If a persisted cart has already been locked by checkout, getOrCreate returns
// an active cart without changing the shopper identity.
const { data: activeCart } = await chaos.cart.getOrCreate(cart.id);
// Cart mutations use the response version as an If-Match precondition. The
// SDK reads and sends it automatically; direct HTTP callers must send the
// current Cart version in the If-Match header.

// Stripe Embedded Checkout — Chaos reserves inventory, locks this Cart, and
// creates the pending Order before Stripe collects the remaining details. The
// response's cart is a separate active Cart for later shopping.
// The return URL must be HTTPS outside local loopback development.
const { data: creation } = await chaos.payments.createEmbeddedCheckoutWithCart(cart.id, {
  email: "shopper@example.com",
  returnUrl: "https://shop.example.com/checkout/success",
});
const session = creation.checkout;
// creation.source_cart is the exact pre-checkout snapshot used for
// InitiateCheckout analytics; creation.cart is the new active Cart.
// Keep using creation.cart for any later shopping; the original Cart is now
// locked and must not be edited or checked out again.
// The source Cart is the recovery key. Retrying the same Cart checkout request
// with the same Cart snapshot reuses the existing Order and Stripe Session.
// The server helper applies this same rule when a response is lost before the
// Cart cookie is rotated; it only creates a new Cart when the source Cart is no
// longer eligible for checkout.
// The SDK derives and sends a stable client idempotency key from the Cart
// snapshot. Chaos derives the provider idempotency key from the Order ID.
const action = session.client_action;
// A storefront can use the SDK's provider adapter from the optional subpath.
// It has no extra dependencies: it loads Stripe.js from https://js.stripe.com
// at runtime (Stripe does not allow bundling it), so nothing else to install.
const mounted = await mountEmbeddedCheckout(action, document.querySelector("#checkout")!, {
  // Optional. `onComplete` fires instead of a redirect only when the Checkout
  // Session uses `redirect_on_completion: "never" | "if_required"`.
  onComplete: () => renderInPlaceSuccess(),
  onAnalyticsEvent: (event) => track(event.eventType),
  // Resume the same session after a reload instead of creating a new one.
  fetchClientSecret: async () => savedClientToken,
});
// `mounted.unmount()` hides the form (e.g. on `onComplete`); `mounted.destroy()`
// disposes it. Direct Stripe accounts do not use a Stripe-Account header.

// PageView, ViewContent, Search, and active ViewDuration are recorded by the
// browser SDK. Cart and checkout resources record their commerce events only
// after the matching operation succeeds. The SDK owns canonical event fields,
// event IDs, attribution, and the /analytics/events transport. Purchase
// remains a payment-confirmation event on the server; the SDK only projects
// the confirmed order to browser providers.
// After the server confirms payment, let the SDK build the canonical projection:
const purchase = toPurchaseAnalyticsInput(order);
if (purchase) chaos.analytics?.purchase(purchase);
// The server remains the source of truth for the ledger Purchase event.
```

For a storefront with a server-rendered framework, use the request-scoped
server adapter and the browser bridge instead of duplicating API paths in page
components:

```ts
import {
  createServerStorefrontClient,
  createStorefrontBrowserClient,
  createProductReviewFromRequest,
  addCartLineFromRequest,
} from "@omnip-org/chaos-js";

// Worker/SSR request — `cookies` is any jar with get(name) and set(name, value, options).
const server = createServerStorefrontClient({
  publishableKey: "pk_...",
  baseUrl: "https://chaos.example/api/v1",
  cookies,
  request,
});

// Browser island — `baseUrl` points to the storefront's thin same-origin routes.
const storefront = createStorefrontBrowserClient({ baseUrl: "/api", analytics: chaos.analytics });
await storefront.cart.addLine(variantId, 1);
await storefront.checkout.createEmbeddedCheckout(returnUrl);
storefront.orders.recordPurchase(purchase);
```

The server helpers `addCartLineFromRequest()`, `updateCartLineFromRequest()`,
`createEmbeddedCheckoutFromRequest()`, `lookupOrderFromRequest()`, and
`createProductReviewFromRequest()` own request parsing, validation, session
cookies, and the corresponding Chaos operation. Framework routes only adapt
the response or redirect. Browser bridge methods own same-origin paths,
credentials, response envelopes, typed errors, and successful cart/checkout
analytics.

### Contract boundary

This package is the canonical Storefront wire contract. A consuming storefront
must import its request helpers, resources, types, and response validation; it
must not duplicate Chaos DTOs, construct equivalent API paths, or cast a raw
response into a local interface. TypeScript generics are not runtime validation,
so resource methods validate payment and other high-risk response shapes before
returning them. When the API contract changes, publish this package first,
update the consumer's lockfile to that exact release, and run the SDK and
consumer checks against the same version.

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
`session_id` column and the UTM values into nullable `utm_*` columns. The
server keeps shopper and internal Order IDs for authoritative payment and
deduplication work; browser checkout events use the public order number and
Cart ID as their association keys. First-touch and last-non-direct traffic
history remains in `properties`; UTM values are not forwarded as Meta custom
parameters.
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

`createStorefrontClient` defaults to same-origin relative URLs (`/api/v1/...`),
which relies on a browser `fetch` and `location`. From Node, an edge
function, or any non-browser environment, pass an absolute `baseUrl`:

```ts
const chaos = createStorefrontClient({
  publishableKey: process.env.CHAOS_PUBLISHABLE_KEY!,
  baseUrl: "https://shop.example.com/api/v1",
});
```

Confirmation emails link to the Sales Channel storefront's `/orders/lookup` page
with the order number and contact email pre-filled as query parameters. The
page submits both in the request body and the API returns the restricted order
view when they match:

```ts
const params = new URLSearchParams(window.location.search);
const order = await chaos.orders.lookupOrder({
  orderNumber: params.get("order_number") ?? "",
  email: params.get("email") ?? "",
});
console.log(order.order_number, order.shipping_status);
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
