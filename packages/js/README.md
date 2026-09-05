# @omnip-org/chaos-js

A typed client for the Chaos Commerce Storefront API, built for a storefront
with its own server-side deployment (SSR/edge/Worker) — the publishable key
and shopper token live server-side only, never in a browser bundle. The
storefront's server uses `createServerStorefrontClient` to talk to Chaos, and
exposes a few thin same-origin routes that its browser code calls through
`createStorefrontBrowserClient`. A bundled browser analytics collector
projects straight to Meta Pixel and GA4; the storefront's own backend can
additionally send Meta Conversions API events using its own Meta access
token, through the separate `@omnip-org/chaos-js/meta-capi` subpath — see
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

This SDK requires a storefront with its own server-side deployment. There is
no supported pure-browser mode: the publishable key and the shopper token are
only ever held by `createServerStorefrontClient`, on the server. A storefront
exposes a few thin same-origin routes backed by the server helpers below, and
its browser code talks only to those routes through
`createStorefrontBrowserClient` — never directly to Chaos.

```ts
// Server: request-scoped, holds the publishable key and the shopper's
// HttpOnly-cookie-backed token. `cookies` is any jar with get(name) and
// set(name, value, options).
import {
  createServerStorefrontClient,
  resolveProductMedia,
} from "@omnip-org/chaos-js";

const chaos = createServerStorefrontClient({
  publishableKey: process.env.CHAOS_PUBLISHABLE_KEY!,
  baseUrl: "https://chaos.example/api/v1",
  cookies,
  request,
});

// Catalog — rendered server-side into the page; no separate client fetch needed.
const { data: products } = await chaos.catalog.listProducts({ q: "shoes" });
const { data: product } = await chaos.catalog.getProduct("running-shoes");

// Product media is returned as compact, reusable rules. Resolve the gallery
// after the shopper selects a Variant: exact Variant, matching Option Value,
// then Product fallback media.
const selectedVariant = product.variants[0]!;
const gallery = resolveProductMedia(product, selectedVariant);
```

```ts
// Storefront backend route handlers (e.g. POST /api/cart/line-items,
// POST /api/checkout) — thin adapters over the server helpers, which own
// request parsing, validation, session cookies, and the Chaos operation:
import {
  addCartLineFromRequest,
  createEmbeddedCheckoutFromRequest,
} from "@omnip-org/chaos-js";

const mutation = await addCartLineFromRequest(chaos, cookies, request);
const creation = await createEmbeddedCheckoutFromRequest(chaos, cookies, request);
// creation.data.source_cart is the exact pre-checkout snapshot used for
// InitiateCheckout analytics; creation.data.cart is the new active Cart —
// keep using it for later shopping, the original Cart is now locked.
// Retrying the same Cart checkout request with the same Cart snapshot reuses
// the existing Order and Stripe Session; a new Cart is only created when the
// source Cart is no longer eligible for checkout.
```

```ts
// Browser: talks only to the storefront's own same-origin routes above.
import {
  createStorefrontAnalytics,
  createStorefrontBrowserClient,
  toPurchaseAnalyticsInput,
} from "@omnip-org/chaos-js";
import { mountEmbeddedCheckout } from "@omnip-org/chaos-js/stripe";

const analytics = createStorefrontAnalytics({
  publishableKey: "pk_...",
  providers: {
    // Browser Meta Pixel ID. The storefront's backend configures the same
    // (or a different) Pixel/dataset ID again via createServerStorefrontClient's
    // `capi` option, for Meta CAPI.
    metaPixel: { pixelId: "1234567890" },
    ga4: { measurementId: "G-EXAMPLE123" },
  },
});
const storefront = createStorefrontBrowserClient({ baseUrl: "/api", analytics });

// Cart/checkout bridge methods own same-origin paths, credentials, response
// envelopes, typed errors, and successful cart/checkout analytics.
const mutation = await storefront.cart.addLine(variantId, 1);
const creation = await storefront.checkout.createEmbeddedCheckout(returnUrl);
// catalog.listProducts/getProduct record Search/ViewContent after a
// successful response — call them from the page that renders search results
// or a PDP if that data came from the server-rendered page, not this fetch.
await storefront.catalog.getProduct("running-shoes");

// Stripe Embedded Checkout — Chaos reserves inventory, locks the Cart, and
// creates the pending Order before Stripe collects the remaining details.
// The return URL must be HTTPS outside local loopback development.
const action = creation.checkout.client_action;
// The SDK's Stripe adapter has no extra dependencies: it loads Stripe.js from
// https://js.stripe.com at runtime (Stripe does not allow bundling it).
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

// Purchase is never inferred from browser activity — only from a confirmed,
// paid order the storefront already has (typically via chaos.orders.lookupOrder
// on the return page):
storefront.orders.recordPurchase(toPurchaseAnalyticsInput(order)!);
```

### Contract boundary

This package is the canonical Storefront wire contract. A consuming storefront
must import its request helpers, resources, types, and response validation; it
must not duplicate Chaos DTOs, construct equivalent API paths, or cast a raw
response into a local interface. TypeScript generics are not runtime validation,
so resource methods validate payment and other high-risk response shapes before
returning them. When the API contract changes, publish this package first,
update the consumer's lockfile to that exact release, and run the SDK and
consumer checks against the same version.

Analytics collection starts immediately when `createStorefrontAnalytics` is
constructed; omit it from `createStorefrontBrowserClient` to skip browser
collection entirely.

The server client automatically acquires and persists the signed shopper
token, in the HttpOnly cookie set up by `createServerStorefrontClient`, used
to associate commerce operations on the first shopper-scoped request.
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

The browser bridge (`StorefrontBrowserClient`) projects `AddToCart`/
`InitiateCheckout` only after the matching request succeeds — the business
request never carries an analytics field — sharing one event ID between the
ledger-free Pixel/GA4 projection and, when a storefront also configured
server-side Meta CAPI (see below), the matching CAPI call. Commerce item
properties use `product_id` and `product_variant_id`. Multi-item events repeat
these fields inside each `items[]` entry; single-item events also expose the
corresponding IDs at the top level. The Meta adapter uses the variant ID as
the Meta content ID when present, otherwise the product ID.

`purchase` is a projection, not a first-party fact: the SDK never infers it
from browser activity, only from a confirmed, paid order the storefront
already has (typically via `chaos.orders.lookupOrder` on a return page). It
derives its event ID from the Order ID, so a reload of the same confirmation
page — and a matching server-side Meta CAPI call for the same order — project
the identical ID and Meta deduplicates the copies.
`storefront.orders.recordPurchase(toPurchaseAnalyticsInput(order))` builds and
sends this projection in one call.

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

Only call `analytics.recordAddToCart`/`recordInitiateCheckout` directly (with
no `eventId`), bypassing the `storefront.cart`/`storefront.checkout` bridge,
when CAPI is **not** also configured for the same action. If both fire for
the same add-to-cart/checkout, always route through the paired helpers above
(or pass the CAPI call's `eventId` through by hand) — two independently-minted
event IDs for one action means Meta receives it twice and cannot deduplicate.

### Guest order lookup

Confirmation emails link to the Sales Channel storefront's `/orders/lookup` page
with the order number and contact email pre-filled as query parameters. The
page submits both in the request body and the API returns the restricted order
view when they match:

```ts
// Browser bridge — the page's route (e.g. /api/orders/lookup) is backed by
// the server's lookupOrderFromRequest() helper.
const params = new URLSearchParams(window.location.search);
const order = await storefront.orders.lookupOrder({
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
