# @omnip-org/chaos-js

A typed browser client for the Chaos Commerce Storefront API. `new` one
`ChaosStorefrontClient` — no storefront backend, proxy, or SSR deployment
required — and dot into `chaos.cart`, `chaos.catalog`, `chaos.payments`,
`chaos.orders`, `chaos.reviews`, `chaos.shopperSession`. The publishable key
is meant to ship in a browser bundle (it's Channel-scoped and read-only); the
shopper token is acquired and persisted automatically (`window.localStorage`
by default).

Client-side event delivery (Meta Pixel, GA4) is wired up internally from
`ClientOptions.events` — there is no separate analytics class to construct,
start, or export. Pass `providers.metaPixel`/`providers.ga4` to turn either
on; omit both to leave event delivery off entirely. `chaos-rust` sends two
events to Meta's server-side Conversions API itself — `InitiateCheckout` at
checkout creation and `Purchase` at payment confirmation — both from the
ad-platform attribution this SDK attaches to the checkout call, and both
deduplicated against this SDK's own Pixel projection by a shared event id.
This package never talks to Meta's CAPI or holds a Meta access token.

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
import { ChaosStorefrontClient, resolveProductMedia } from "@omnip-org/chaos-js";
import { mountEmbeddedCheckout } from "@omnip-org/chaos-js/stripe";

const chaos = new ChaosStorefrontClient({
  publishableKey: "pk_...",
  baseUrl: "https://chaos.example.com/api/v1",
  events: {
    providers: {
      metaPixel: { pixelId: "1234567890" },
      ga4: { measurementId: "G-EXAMPLE123" },
    },
  },
});

// Catalog reads record Search/ViewContent to the configured providers.
const { data: products } = await chaos.catalog.listProducts({ q: "shoes" });
const { data: product } = await chaos.catalog.getProduct("running-shoes");

// Product media is returned as compact, reusable rules. Resolve the gallery
// after the shopper selects a Variant: exact Variant, matching Option Value,
// then Product fallback media.
const selectedVariant = product.variants[0]!;
const gallery = resolveProductMedia(product, selectedVariant);

// Cart mutations project AddToCart automatically.
const cart = await chaos.cart.getOrCreate();
await chaos.cart.addLine(cart.data.id, selectedVariant.id, 1);

// Checkout: fbc/fbp and the current page URL are read automatically (pass
// `attribution` explicitly to override, or `{}` to send none). Chaos sends
// this straight to Meta CAPI as `InitiateCheckout`, stores it on the Cart,
// and replays it as `Purchase` once the order is paid — the only two
// server-side conversion events Chaos ever sends.
const creation = await chaos.payments.createEmbeddedCheckoutWithCart(cart.data.id, {
  returnUrl: "https://shop.example.com/checkout/return",
});
// `creation.data.checkout.event_id` is the InitiateCheckout CAPI event id —
// this SDK already reuses it for the Pixel projection automatically
// (`recordCheckoutCreation`, called internally above) so Meta deduplicates
// the two; reach for it yourself only if you fire InitiateCheckout by hand.

// Stripe Embedded Checkout — Chaos reserves inventory, locks the Cart, and
// creates the pending Order before Stripe collects the remaining details.
// The return URL must be HTTPS outside local loopback development.
const action = creation.data.checkout.client_action;
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

// Purchase (Pixel/GA4) is never inferred from browser activity — only from a
// confirmed, paid order, typically on the return page right after lookupOrder:
chaos.orders.recordConfirmedPurchase(order);
```

### Contract boundary

This package is the canonical Storefront wire contract. A consuming
storefront must use the exported wire types and `ChaosStorefrontClient` for
every Chaos-facing operation; it must not duplicate Chaos DTOs, construct
equivalent API paths, or cast a raw response into a local interface.
TypeScript generics are not runtime validation, so resource methods validate
payment and other high-risk response shapes before returning them. When the
API contract changes, publish this package first, update the consumer's
lockfile to that exact release, and run the SDK and consumer checks against
the same version.

Event delivery starts as soon as `ChaosStorefrontClient` is constructed with
an `events` option; a destination (Pixel, GA4) stays off until its config key
is present in `events.providers` — there is no separate start/stop call.
Advanced/uncommon operations (`getShopperToken`, `randomUUID`, the raw
`cart.getActive()`/`cart.getOrCreate()`) live directly on the client; invalid
shopper-token retries are opt-in (`retryInvalidShopperToken`) because
silently minting a replacement can orphan a cart or hide an order.

There are exactly six events — `page_view`, `view_content`, `search`,
`add_to_cart`, `initiate_checkout`, `purchase` — and this SDK is the only
thing that ever emits them client-side; there is no store-facing
custom-event API. All six project straight to the configured Meta Pixel and
GA4 as they happen — there is no queue, no batching, and no chaos-owned
analytics ledger; provider scripts are optional and load immediately when
configured. GA4 automatic PageView collection stays disabled; Chaos maps
semantic events to GA4 ecommerce names.

`chaos.cart`/`chaos.catalog`/`chaos.payments` project `AddToCart`/
`Search`/`ViewContent`/`InitiateCheckout` automatically after the matching
request succeeds — route every mutation through them rather than the raw
`chaos.request` escape hatch, or the matching event is silently skipped.
Commerce item inputs retain `product_id` and `product_variant_id`; built-in
Meta Pixel and GA4 commerce projections use `product_variant_id` as the
item/content ID, and `view_content` falls back to `product_id` when no
variant is supplied.

`purchase` is a projection, not a first-party fact: the SDK never infers it
from browser activity, only from a confirmed, paid order the storefront
already has (typically via `chaos.orders.lookupOrder` on a return page). It
derives its event ID from the Order ID, so a reload of the same confirmation
page projects the identical ID and Meta deduplicates the copies.
`chaos.orders.recordConfirmedPurchase(order)` (also reachable as
`chaos.recordConfirmedPurchase(order)`) builds and sends this projection in
one call from the shape `lookupOrder` already returns.

The collector maintains a first-party `_fbc` cookie from a landing `fbclid`,
bounded and capped at 90 days, independent of whether the Meta Pixel script
has finished loading. `chaos.payments.createEmbeddedCheckout*` reads this
same `_fbc` cookie (and Pixel's own `_fbp` cookie) by default when building
the checkout's `attribution` — see below.

### Server-side Meta Conversions API

There is nothing to configure in this package for CAPI: `chaos-rust` sends
`InitiateCheckout` at checkout creation and `Purchase` at payment
confirmation, using whatever ad-platform attribution was attached to the
checkout call. Pass it explicitly to override the `_fbc`/`_fbp`/page-URL
defaults, or send none:

```ts
await chaos.payments.createEmbeddedCheckoutWithCart(cart.data.id, {
  returnUrl: "https://shop.example.com/checkout/return",
  attribution: { meta: { fbc: readFbcSomeOtherWay() } },
  // attribution: {}, // send no attribution at all
});
```

`chaos-rust` re-validates and bounds whatever it stores, and drops anything
malformed rather than failing the checkout over it — attribution is
enrichment for ad platforms, never a condition of a successful purchase.

### Guest order lookup

Confirmation emails link to the Sales Channel storefront's `/orders/lookup`
page with the order number and contact email pre-filled as query parameters.
The page submits both and the API returns the restricted order view when
they match:

```ts
const params = new URLSearchParams(window.location.search);
const order = await chaos.orders.lookupOrder({
  orderNumber: params.get("order_number") ?? "",
  email: params.get("email") ?? "",
});
console.log(order.data.order_number, order.data.shipping_status);
chaos.orders.recordConfirmedPurchase(order.data);
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
