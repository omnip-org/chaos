# @omnip-org/chaos-js

A typed browser client for the Chaos Commerce Storefront API. `new` one
`ChaosStorefrontClient` — no storefront backend, proxy, or SSR deployment
required — and dot into `chaos.cart`, `chaos.catalog`, `chaos.payments`,
`chaos.orders`, `chaos.reviews`, `chaos.shopperSession`. The publishable key
is meant to ship in a browser bundle (it's Channel-scoped and read-only); the
shopper token is acquired and persisted automatically (`window.localStorage`
by default).

Every Storefront request sends the channel-scoped publishable key through
`X-Chaos-Publishable-Key`. Shopper-owned requests additionally send the
shopper token through `X-Chaos-Shopper-Token`. Storefront requests do not use
the standard `Authorization` header.

Client-side event delivery (Meta Pixel, GA4) is wired up internally from
`ClientOptions.events` — there is no separate analytics class to construct,
start, or export. Pass `providers.metaPixel`/`providers.ga4` to turn either
on; omit both to leave event delivery off entirely. `chaos-rust` sends one
event to Meta's server-side Conversions API itself — `Purchase` at payment
confirmation. The SDK attaches ad-platform attribution only to checkout;
Chaos saves that snapshot on the Cart and applies it to the later Purchase.
The browser Purchase uses the same Order id so Meta can deduplicate it. This
package never talks to Meta's CAPI or holds a Meta access token.

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

// Call once on page load (don't await it on the critical render path):
// acquires the shopper session and resolves its active cart from the server,
// so the first "add to cart" is a single PUT instead of
// session + create + get + put. Concurrent calls share one round of work.
const cart = await chaos.cart.warmup();

// Catalog reads record Search/ViewContent to the configured providers.
const { data: products } = await chaos.catalog.listProducts({ q: "shoes" });
const { data: product } = await chaos.catalog.getProduct("running-shoes");

// Product media is returned as compact, reusable rules. Resolve the gallery
// after the shopper selects a Variant: exact Variant, matching Option Value,
// then Product fallback media.
const selectedVariant = product.variants[0]!;
const gallery = resolveProductMedia(product, selectedVariant);

// Cart mutations project AddToCart to the configured browser providers
// automatically. The API request contains only cart data; AddToCart is not
// sent through server-side CAPI. addLine/setLine/removeLine reuse the last
// cart body the client saw (within `cartSnapshotTtlMs`, default 30s) instead of a
// separate GET; pass `cartSnapshotTtlMs: 0` to force a re-read every time. If
// the cart id has since been locked or completed by a checkout, the mutation
// retries once against the shopper's current active cart — read the id back
// from the response, it may have changed.
const { data: activeCart } = await chaos.cart.addLine(
  cart.data.id,
  selectedVariant.id,
  1,
);

// Checkout: fbc/fbp and the current page URL are read automatically (pass
// `attribution` explicitly to override, or `{}` to send none). Chaos sends
// the attribution on the Cart and uses it for Meta CAPI `Purchase` once the
// order is paid. InitiateCheckout is projected only by the browser SDK.
const creation = await chaos.payments.createEmbeddedCheckoutWithCart(activeCart.id, {
  returnUrl: "https://shop.example.com/checkout/return",
});

// Stripe Embedded Checkout — Chaos reserves inventory, locks the Cart, and
// creates the pending Order before Stripe collects the remaining details.
// The return URL must be HTTPS outside local loopback development.
const action = creation.data.checkout.client_action;
const nextCart = creation.data.cart;
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

// A latency-sensitive checkout page that already has a fresh Cart body can
// call createEmbeddedCheckoutFromCart(activeCart, options), mount its returned
// action immediately, and resolve chaos.cart.getOrCreate() in parallel.

// On the return page, Stripe has appended order_id to the URL. Poll the
// shopper-owned Order until Chaos's payment webhook marks it paid. Each read
// automatically attempts Pixel and GA4 Purchase for this fresh checkout.
const orderId = new URLSearchParams(location.search).get("order_id")!;
const { data: order } = await chaos.orders.getCheckoutOrder(orderId);
if (order.status === "confirmed" && order.payment_status === "paid") showSuccess();
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

Client recovery decisions use stable API error codes rather than broad HTTP
statuses. Authentication distinguishes `publishable_key_required`,
`publishable_key_invalid`, `shopper_token_required`, and
`shopper_token_invalid`; a missing Cart is `cart_not_found`. Only
`shopper_token_invalid` may rotate shopper identity, and only
`cart_not_found` may trigger Cart creation. Other 401, 403, and 404 responses
remain visible to the caller.

Shopper-owned responses and issued credentials use `Cache-Control: private,
no-store`. Storefront responses also vary on both Chaos credential headers so
a shared HTTP cache cannot reuse one Channel or Shopper response for another.
Treat `shopper_token` as opaque; `shopper_id` is the explicit analytics and
identity value and may be stored beside it by this SDK.

Event delivery starts as soon as `ChaosStorefrontClient` is constructed with
an `events` option; a destination (Pixel, GA4) stays off until its config key
is present in `events.providers` — there is no separate start/stop call.
Advanced/uncommon operations (`getShopperToken`, `randomUUID`, the raw
`cart.getCurrent()`/`cart.getActive()`/`cart.getOrCreate()`) live directly on
the client; invalid shopper-token retries are opt-in
(`retryInvalidShopperToken`) because
silently minting a replacement can orphan a cart or hide an order.
`cart.warmup()` is the intended page-load call; it delegates to
`cart.resume()`, which asks `GET /carts` for an existing shopper's current
active Cart and creates one only when none exists. A newly minted shopper goes
straight to creation. Cart identity is server-owned and is not persisted in
browser storage. A consumer that already has a cart id can keep calling
`cart.getOrCreate(id)` and ignore both.

There are exactly six events — `page_view`, `view_content`, `search`,
`add_to_cart`, `initiate_checkout`, `purchase` — and this SDK is the only
thing that ever emits them client-side; there is no store-facing
custom-event API. Five of the six project straight to the configured Meta
Pixel and GA4 as they happen — there is no queue, no batching, and no
chaos-owned analytics ledger; provider scripts are optional and load
immediately when configured. `page_view` is GA4-only: it is not one of
Meta's Standard Events (the base Pixel snippet fires it for traffic
counting, but it isn't part of the commerce funnel Meta optimizes ads
against the way `view_content`/`add_to_cart`/`purchase` are), so this SDK
never sends it to Meta Pixel. GA4 automatic PageView collection stays
disabled; Chaos maps semantic events to GA4 ecommerce names.

`chaos.cart`/`chaos.catalog`/`chaos.payments` project `AddToCart`/
`Search`/`ViewContent`/`InitiateCheckout` automatically after the matching
request succeeds — route every mutation through them rather than the raw
`chaos.request` escape hatch, or the matching event is silently skipped.
Commerce item inputs retain `product_id` and `product_variant_id`; built-in
Meta Pixel and GA4 commerce projections use `product_variant_id` as the
item/content ID, and `view_content` falls back to `product_id` when no
variant is supplied. AddToCart and InitiateCheckout mint their event ids in
the browser. Purchase uses the Order id, shared with the server-side CAPI
copy for Meta deduplication.

`catalog.getProduct`'s automatic `ViewContent` only knows the product's
*first* variant — it has no way to know which one the shopper will actually
see, since picking a different variant on the page (a color swatch, a size
selector) is a local UI state change with no further request for the SDK to
hook into. Call `chaos.recordViewContent` again once the shopper picks one:

```ts
chaos.recordViewContent({
  productId: product.id,
  productVariantId: selectedVariant.id,
  priceMinor: selectedVariant.price.amount_minor,
  currency: selectedVariant.price.currency,
});
```

Skipping this leaves every `ViewContent` at the product level while
`AddToCart`/`Purchase` report at the variant level, which breaks Meta's
catalog matching for dynamic ads and "viewed but not bought" retargeting on
any product with more than one variant. `priceMinor`/`currency` are required
on every call, validated the same way `AddToCart`/`Purchase` already are.

`purchase` is a projection of a server-confirmed Order. Checkout creation
records the Order UUID in this tab; `orders.getCheckoutOrder` uses
the existing shopper token to read the saved Order and automatically sends
Pixel/GA4 Purchase only for that fresh checkout after payment is confirmed.
The authenticated Order read returns the current `orders` row with its flat
contact and address columns, plus related lines and fulfillment progress; it
does not use a checkout-time snapshot.
The Order UUID is the Meta event ID and GA4 transaction ID. The SDK keeps
separate per-provider dedup records so a failed provider can retry without
repeating the other. It supplies saved Order identity to Meta Pixel Advanced
Matching, while GA4 receives the net item amount, tax, and shipping without
email or address. The manual `recordConfirmedPurchase` method remains for
integrations that already have an authoritative Order, but order-history
views should not call it.

The collector maintains a first-party `_fbc` cookie from a landing `fbclid`,
bounded and capped at 90 days, independent of whether the Meta Pixel script
has finished loading. `chaos.payments.createEmbeddedCheckout*` reads this
same `_fbc` cookie (and Pixel's own `_fbp` cookie) by default when building
the checkout request `attribution` — see below.

### Server-side Meta Conversions API

There is nothing to configure in this package for CAPI: `chaos-rust` sends
only `Purchase` at payment confirmation, using the attribution captured by
the checkout call. Pass it explicitly to override the
`_fbc`/`_fbp`/page-URL defaults, or send none:

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

`page_view`, `view_content`, `search`, `add_to_cart`, and
`initiate_checkout` never reach CAPI; only their browser Pixel/GA4
projections carry them. `Purchase` is the server-side event because payment
confirmation is authoritative on the server and must survive a closed return
page or blocked browser script.

### Guest order lookup

Confirmation emails link to the Sales Channel storefront's `/orders/details`
page with the order number and contact email pre-filled as query parameters.
The page sends both to `GET /orders/search` and the API returns the restricted order view when
they match:

```ts
const params = new URLSearchParams(window.location.search);
const order = await chaos.orders.lookupOrder({
  orderNumber: params.get("order_number") ?? "",
  email: params.get("email") ?? "",
});
console.log(order.data.order_number, order.data.fulfillment_status);
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

`src/client.ts` is the public facade and coordinates the typed resource
classes. Its stateful infrastructure is kept under `src/internal/`:

- `transport.ts` owns HTTP headers, URLs, JSON and API error decoding.
- `shopper-session.ts` owns the shopper credential, attribution and browser
  persistence.
- `storefront-events.ts` owns Pixel/GA4 projection and checkout-return markers.

Resource classes contain Storefront operations and recovery rules; they do
not construct authentication headers or access browser storage directly.

```sh
npm run build --prefix packages/js
npm test --prefix packages/js
```

Types in `src/types.ts` are the hand-written Storefront wire contract; keep them
in sync with the public routes and response handlers when the Store API changes.
