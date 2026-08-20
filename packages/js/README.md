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

const chaos = createStorefrontClient({ publishableKey: "pk_live_..." });

// Catalog
const { data: products } = await chaos.catalog.listProducts({ q: "shoes" });
const { data: product } = await chaos.catalog.getProduct("running-shoes");

// Cart — the shopper token is acquired and persisted automatically on the
// first mutating call, then reused for every subsequent Cart/Checkout call.
const { data: cart } = await chaos.cart.create();
await chaos.cart.setLine(cart.id, product.variants[0].id, { quantity: 1 });

// Checkout and Order
const { data: checkout } = await chaos.checkout.create(cart.id, {
  contact: { email: "shopper@example.com" },
  billing_address: { full_name: "Ada Lovelace", address_line1: "1 Main St", locality: "London", country_code: "GB" },
});
const { data: order } = await chaos.checkout.createOrder(checkout.id);

// Analytics (bundled — no separate package)
chaos.analytics?.setConsent({ analyticsStorage: true, advertisingStorage: false, policyVersion: "cmp-2026-08" });
chaos.analytics?.start();
chaos.analytics?.pageViewed();
```

Pass `analytics: false` to `createStorefrontClient` to skip constructing the
collector entirely.

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
