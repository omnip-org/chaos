import { createStorefrontClient, type ChaosStorefrontClient } from "@omnip-org/chaos-js";

/**
 * Creates a Chaos storefront client. Works in both SSR (Astro server
 * runtime) and the browser (React islands), with two differences forced by
 * the runtime rather than by choice:
 *
 * - Base URL: in the browser, `fetch` resolves relative URLs against the
 *   current page origin, so the SDK's default `/store/v1` works
 *   unmodified. Node's `fetch` has no notion of a page origin and throws
 *   on a relative URL ("Failed to parse URL from /store/v1/..."), so
 *   PUBLIC_CHAOS_STORE_API_BASE_URL is required for SSR pages. It's the
 *   same variable the browser build reads, so one env var covers both.
 * - Analytics: the bundled collector's constructor requires `document`/
 *   `window`, which don't exist under Node — pass `analytics: false` on
 *   the server. Cart/checkout pages that run in the browser omit this and
 *   get the collector as usual.
 *
 * The Store is determined entirely by the publishable key configured below.
 */
export function createChaosClient(): ChaosStorefrontClient {
  const baseUrl = import.meta.env.PUBLIC_CHAOS_STORE_API_BASE_URL;
  if (import.meta.env.SSR && !baseUrl) {
    throw new Error(
      "PUBLIC_CHAOS_STORE_API_BASE_URL is required for server-rendered pages (Node's fetch cannot resolve a relative URL). Set it to your Store API origin, e.g. https://shop.example.com/store/v1.",
    );
  }
  return createStorefrontClient({
    publishableKey: import.meta.env.PUBLIC_CHAOS_PUBLISHABLE_KEY,
    baseUrl,
    analytics: import.meta.env.SSR ? false : undefined,
  });
}
