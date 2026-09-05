# @omnip-org/chaos-js

A typed client for the Chaos Commerce Storefront API — the
public-key-authenticated surface meant to be called directly from storefront
browsers — with a bundled browser analytics collector that projects straight
to Meta Pixel and GA4. The SDK is the typed public contract. One SDK covers
catalog browsing, cart, checkout, order, payment, and behavior analytics flows.
A storefront with its own server-side deployment can additionally send Meta
Conversions API events from its own backend, using its own Meta access token,
through the separate `@omnip-org/chaos-js/meta-capi` subpath — see
[Server-side Meta Conversions API](#server-side-meta-conversions-api).

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
      // Browser Meta Pixel ID. A storefront with its own server-side
      // deployment configures the same (or a different) Pixel/dataset ID
      // again via createServerStorefrontClient's `capi` option, for Meta CAPI.
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

// PageView, ViewContent, and Search project straight to the configured Meta
// Pixel and GA4 as they happen. Cart and checkout resources project
// AddToCart/InitiateCheckout only after the matching operation succeeds; the
// business request never carries an analytics field. Purchase is never
// inferred from browser activity — only from a confirmed, paid order the
// storefront already has.
// After the server confirms payment, let the SDK build the canonical projection:
const purchase = toPurchaseAnalyticsInput(order);
if (purchase) chaos.analytics?.recordPurchase(purchase);
// A storefront with its own server-side deployment can also send the same
// order to Meta CAPI from its backend — see the meta-capi subpath below.
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
browser collector entirely. Collection starts immediately when the analytics
client is constructed.

The client automatically acquires and persists the signed shopper token used to
associate commerce operations on the first shopper-scoped request.
`cart.getActive()` reads only an active cart, while `cart.getOrCreate()`
explicitly recovers from a missing or completed cart. Invalid shopper-token
retries are opt-in because silently minting a replacement can orphan a cart or
hide an order.

There are exactly six events — `page_view`, `view_content`, `search`,
`add_to_cart`, `initiate_checkout`, `purchase` — and this SDK is the only
thing that ever emits them; there is no store-facing custom-event API.
PageView, ViewContent, and Search project straight to the configured Meta
Pixel and GA4 as they happen — there is no queue, no batching, and no
chaos-owned analytics ledger; provider scripts are optional and load
immediately when configured. GA4 automatic PageView collection stays
disabled; Chaos maps semantic events to GA4 ecommerce names.

Cart and checkout resources project `AddToCart`/`InitiateCheckout` only after
the matching request succeeds — the business request never carries an
analytics field — sharing one event ID between the ledger-free Pixel/GA4
projection and, when a storefront also configured server-side Meta CAPI (see
below), the matching CAPI call. Commerce item properties use `product_id` and
`product_variant_id`. Multi-item events repeat these fields inside each
`items[]` entry; single-item events also expose the corresponding IDs at the
top level. The Meta adapter uses the variant ID as the Meta content ID when
present, otherwise the product ID.

`purchase` is a projection, not a first-party fact: the SDK never infers it
from browser activity, only from a confirmed, paid order the storefront
already has (typically via `chaos.orders.lookupOrder` on a return page). It
derives its event ID from the Order ID, so a reload of the same confirmation
page — and a matching server-side Meta CAPI call for the same order — project
the identical ID and Meta deduplicates the copies.
`chaos.analytics?.recordConfirmedPurchase(order)` builds and sends this
projection in one call.

The collector maintains a first-party `_fbc` cookie from a landing `fbclid`,
bounded and capped at 90 days, independent of whether the Meta Pixel script
has finished loading — a server-side Meta CAPI call later in the same visit
reads that cookie for `user_data.fbc`.

### Server-side Meta Conversions API

Meta CAPI delivery holds the store's own Meta access token, so it lives
outside the main SDK entry, as the separate `@omnip-org/chaos-js/meta-capi`
subpath — import it only from server-side code, never from a browser bundle:

```ts
import {
  createServerStorefrontClient,
  recordConfirmedPurchaseCapi,
} from "@omnip-org/chaos-js";

const chaos = createServerStorefrontClient({
  publishableKey: process.env.CHAOS_PUBLISHABLE_KEY!,
  baseUrl: "https://shop.example.com/api/v1",
  cookies,
  request,
  // Provide the store's own Meta access token from this deployment's
  // environment variables. Chaos never stores or proxies this secret.
  capi: {
    meta: {
      accessToken: process.env.META_CAPI_ACCESS_TOKEN!,
      pixelId: process.env.META_PIXEL_ID!,
      testEventCode: process.env.META_TEST_EVENT_CODE,
    },
  },
});

// addCartLine/updateCartLine/createEmbeddedCheckoutFromRequest already send
// Meta CAPI when `capi` is configured, and return the shared `event_id` on
// the mutation/checkout result so a browser Pixel projection reading the
// same response can reuse it instead of minting a second one.

// On the order-confirmation page, once the order is confirmed and paid:
await recordConfirmedPurchaseCapi(chaos, cookies, order, request.url);
```

A store with no server-side deployment simply omits `capi` and gets Pixel +
GA4 only — an intentional fallback, not a missing feature. There is no
chaos-owned analytics ledger or admin-side event browser; a store that wants
its own first-party record of behavior events owns that storage itself.

CAPI delivery is fire-and-forget and fails silently by default (a delivery
failure must never turn a successful commerce operation into a failed one).
Pass `capi.meta.onError(error, event)` to observe failures — e.g. an expired
access token — instead of them going unnoticed.

Only call `chaos.analytics?.recordAddToCart`/`recordInitiateCheckout`
directly (with no `eventId`) when CAPI is **not** also configured for the
same action. If both fire for the same add-to-cart/checkout, always route
through the paired helpers above (or pass the CAPI call's `eventId` through
by hand) — two independently-minted event IDs for one action means Meta
receives it twice and cannot deduplicate.

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
