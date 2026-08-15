# Chaos Storefront Analytics

This dependency-free ES module collects the allowlisted version-1 browser events accepted by the Chaos Store API. It is first-party infrastructure: it does not load advertising scripts, read cookies, or infer consent.

```js
import { createStorefrontAnalytics } from "@chaos-commerce/storefront-analytics";

const analytics = createStorefrontAnalytics({
  publishableKey: "pk_live_...",
});

analytics.setConsent({
  analyticsStorage: true,
  advertisingStorage: false,
  policyVersion: "cmp-2026-08",
});
analytics.start();
analytics.pageViewed();
```

Call the semantic methods only after the underlying Storefront action occurs. Browser events remain untrusted observations; never derive a commercial amount or successful Order, Payment, Refund, Return, or Fulfillment state from them.

`start()` measures engagement only while the document is visible and its window has focus. Heartbeats are split into intervals of at most 60 seconds. `pagehide` and `stop()` attempt a final `fetch` with `keepalive`; missing final heartbeats are expected. Failed batches remain in the bounded in-memory queue with their original event IDs so server-side deduplication remains effective.

The SDK stores a random anonymous ID in `localStorage` and a random session ID in `sessionStorage`. It sends neither value until analytics-storage consent is granted. Revocation drops unsent events and stops future engagement collection. Identity linkage, retention, deletion, and advertising eligibility remain server policy decisions.

Run the dependency-free test suite with:

```text
npm test --prefix packages/storefront-analytics
```
