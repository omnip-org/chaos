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

// The first successful page of a non-empty query records one Search event.
const { data: products } = await chaos.catalog.listProducts({ q: "shoes" });
// `openProduct` is the detail-page operation. It records Product-level
// ViewContent/view_item internally, including when every Variant is sold out.
// Use the pure `getProduct` read instead for SSR, prefetching or cache warming.
const { data: product } = await chaos.catalog.openProduct("running-shoes");

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

// If the response is lost, retry with the same Cart and return URL. The API
// recovers its pending Order and provider session, including after a reload.

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
// automatically attempts Pixel and GA4 Purchase for a confirmed paid Order.
const orderId = new URLSearchParams(location.search).get("order_id")!;
const { data: order } = await chaos.orders.waitForCheckoutOrder(orderId);
if (order.status === "confirmed" && order.payment_status === "paid") showSuccess();
```

### Browser and server runtimes

The client can perform pure Storefront API reads during SSR, but its shopper,
checkout and analytics behavior is browser-oriented. In a browser it can use
`localStorage`, the current URL, first-party cookies and the DOM. It can
therefore persist a shopper, capture checkout attribution, load
Pixel/GA4 and mount Stripe Embedded Checkout.

On a server, create a client per incoming request. Do not share one client as
a process singleton: it holds the current shopper token, cart snapshots and
checkout idempotency keys in memory. Use an absolute `baseUrl`, leave `events`
unset, disable browser persistence and seed a request-owned shopper token only
when the operation needs one:

```ts
const chaos = new ChaosStorefrontClient({
  publishableKey: process.env.CHAOS_PUBLISHABLE_KEY!,
  baseUrl: "https://chaos.example.com/api/v1",
  storage: null,
  autoAcquireShopperToken: false,
});

if (shopperTokenFromThisRequest) {
  chaos.setShopperToken(shopperTokenFromThisRequest);
}
const product = await chaos.catalog.getProduct(productSlug);
```

The SDK never falls back to a server runtime's process-wide `localStorage`.
Server execution also has no browser cookies or page URL, so it cannot
automatically collect `_fbc`, `_fbp`, UTM parameters or
`source_url`, and Pixel/GA4 delivery is unavailable. Keep checkout and the
shopper-facing event-producing resource calls in the browser. In particular,
a server-originated checkout request would otherwise identify the application
server's network and user-agent context instead of the shopper's browser.

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
Treat `shopper_token` as opaque. The SDK stores its `shopper_id` beside it and
hashes that id for Meta's `external_id` matching. It does not use the anonymous
shopper id as GA4 User-ID, which is reserved for an authenticated account id.

Chaos-owned browser keys use one versioned namespace scoped by API base URL and
publishable key:

```text
chaos.storefront.v1.<scope>.shopper.token
chaos.storefront.v1.<scope>.shopper.id
chaos.storefront.v1.<scope>.attribution.utm.first
```

The shopper identity and first-touch UTM use `ClientOptions.storage`
(`localStorage` by default). Cart ids, Order ids and analytics event ids are
not persisted.

The first-touch UTM snapshot only bridges a landing page to lazy shopper
creation after an MPA navigation. It feeds Chaos's own shopper acquisition
record and is not supplied to GA4 or Meta CAPI. The browser Google tag collects
campaign parameters from the landing-page URL independently; Meta CAPI uses
`_fbc`, `_fbp`, `source_url`, request context and order identity. Checkout does
not persist or replay a last-touch UTM snapshot. A caller with a separate
first-party attribution model can still pass `options.attribution.utm`
explicitly.

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

The storefront funnel has six signals: `page_view`, `view_content`, `search`,
`add_to_cart`, `initiate_checkout`, and `purchase`. The Google tag collects
GA4 PageView automatically, including browser-history changes when that option
is enabled in Enhanced Measurement. Chaos does not implement or send PageView
itself, and it never sends Meta `PageView`. The other five events are projected
by the SDK with no store-facing custom-event API. There is no queue, batching,
or Chaos-owned analytics ledger; provider scripts are optional and load
immediately when configured.

| Storefront event | Trigger | Meta Pixel | GA4 | Meta CAPI |
| --- | --- | --- | --- | --- |
| Page view | Google tag automatic collection | — | `page_view` | — |
| Product view | Successful `catalog.openProduct` | `ViewContent` | `view_item` | — |
| Search | Successful first page with a non-empty `q` | `Search` | `search` | — |
| Cart addition | Successful line mutation whose quantity increased | `AddToCart` | `add_to_cart` | — |
| Checkout start | Successful embedded checkout creation or recovery | `InitiateCheckout` | `begin_checkout` | — |
| Purchase | Confirmed paid `orders.waitForCheckoutOrder` response | `Purchase` | `purchase` | `Purchase` at payment confirmation |

Browser commerce delivery is best effort and never changes the Storefront
operation's result. It does not keep a local event ledger. Repeated operations
are sent again with stable identifiers where the provider supports them.

`chaos.cart` and `chaos.payments` project `AddToCart` and
`InitiateCheckout` automatically after the matching request succeeds;
the first successful page of a non-empty catalog query projects `Search`;
cursor pagination does not. Route those operations through the typed resources
rather than the raw `chaos.request` escape hatch, or the matching event is
skipped.
Commerce item inputs retain `product_id` and `product_variant_id`. Every Meta
commerce event uses `product_id` for `content_ids` and `contents[].id`, keeping
the catalog identity stable from ViewContent through Purchase. GA4 uses the
same Product ID as `item_id` and keeps the selected Variant title in
`item_variant`. AddToCart mints its event id in the browser.
InitiateCheckout and Purchase use the Order UUID as Meta's event ID. A repeated
checkout entry may produce another GA4 `begin_checkout`, which has no standard
transaction ID. Browser Purchase also uses the Order UUID as GA4's
`transaction_id` and shares its Meta event ID with the server-side CAPI copy.

`catalog.getProduct` is a pure read because a successful request does not
prove that its details reached the screen: it may be an SSR load, route
prefetch, cache fill, or discarded render. Product detail pages use
`catalog.openProduct` instead. It returns the same Product response and emits
one Product-level `ViewContent`/`view_item` internally:

```ts
const { data: product } = await chaos.catalog.openProduct("running-shoes");
```

The event identifies the view only with `product.id`; it does not select a
Variant, depend on inventory, or invent a Product price. Viewing a sold-out
Product still counts. Variant changes stay local UI state and do not emit
another product view. There is no public event method for the storefront to call.

`purchase` is a projection of a server-confirmed Order.
`orders.waitForCheckoutOrder` uses the existing shopper token and polls the
saved Order until it is paid or reaches a failed, expired or cancelled state.
It defaults to a one-second interval and a 30-second timeout and accepts an
`AbortSignal`. A confirmed paid response attempts Pixel/GA4 Purchase before it
is returned (including partially refunded or refunded after an earlier
payment). `orders.getCheckoutOrder` remains available for a single read.
The authenticated Order read returns the current `orders` row with its flat
contact and address columns, plus related lines and fulfillment progress; it
does not use a checkout-time snapshot.
The Order UUID is the Meta event ID and GA4 transaction ID, so Meta can merge
the browser and CAPI copies and GA4 can deduplicate repeated Purchase events.
The SDK supplies saved Order identity to Meta Pixel Advanced Matching, while
GA4 receives the net item amount, tax, and shipping without email or address.
Order history and guest lookup reads never emit Purchase.

Meta's browser and server transports express customer matching differently.
The Pixel receives SHA-256 order identity through the customer-data argument
of `fbq("init", pixelId, customerData)` before Purchase; the Purchase event
parameters contain only commerce data. CAPI places the same hashed identity,
plus `fbc`, `fbp`, client IP and user agent, in `user_data`, while its
`custom_data` contains the order, value, currency and items. Customer identity
is never copied into either Purchase `custom_data` object.

When a landing URL contains `fbclid`, the client maintains Meta's standard
first-party `_fbc` cookie for up to 90 days, even when browser event providers
are omitted. It reuses a matching cookie and keeps no additional Chaos cache.
`chaos.payments.createEmbeddedCheckout*` reads this same `_fbc` cookie (and
Pixel's own `_fbp` cookie) by default when building the checkout request
`attribution` — see below.

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
- `browser-storage.ts` owns versioned, store-scoped browser key names.
- `storefront-events.ts` owns Pixel/GA4 projection.

Resource classes contain Storefront operations and recovery rules; they do
not construct authentication headers or access browser storage directly.

```sh
npm run build --prefix packages/js
npm test --prefix packages/js
```

Types in `src/types.ts` are the hand-written Storefront wire contract; keep them
in sync with the public routes and response handlers when the Store API changes.
