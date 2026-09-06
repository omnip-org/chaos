import type { CheckoutAttribution } from "../types.js";
import { readUtmTags } from "./utm.js";

/**
 * Ad-platform attribution read off the browser's own cookies and URL, shared
 * by the checkout call (Meta CAPI `InitiateCheckout`) and cart-line additions
 * (Meta CAPI `AddToCart`). Meta's Pixel install sets `_fbp` itself; `_fbc` is
 * chaos-js's own copy of the `fbclid` URL param (see `events/browser.ts`'s
 * `maintainFbcCookie`). Both are plain, non-HttpOnly cookies by Meta's own
 * design, so reading them here needs no extra wiring. `source_url` is the
 * current page URL — chaos-rust forwards it as the CAPI `event_source_url`.
 * @internal
 */
export function defaultAdAttribution(): CheckoutAttribution {
  const fbc = readCookie("_fbc");
  const fbp = readCookie("_fbp");
  const sourceUrl =
    typeof window === "undefined" ? undefined : window.location.href;
  const utm = readUtmTags();
  return {
    ...(sourceUrl && { source_url: sourceUrl }),
    ...(utm && { utm }),
    ...((fbc || fbp) && { meta: { ...(fbc && { fbc }), ...(fbp && { fbp }) } }),
  };
}

/** `true` when the attribution carries anything worth sending. @internal */
export function hasAdAttribution(attribution: CheckoutAttribution): boolean {
  return Boolean(
    attribution.source_url ||
      (attribution.utm && Object.keys(attribution.utm).length > 0) ||
      (attribution.meta && (attribution.meta.fbc || attribution.meta.fbp)),
  );
}

/**
 * `{ attribution }` when the browser had anything to attach, else `{}` — spread
 * into a request body so an empty attribution is simply omitted. @internal
 */
export function adAttributionBody():
  | { attribution: CheckoutAttribution }
  | Record<string, never> {
  const attribution = defaultAdAttribution();
  return hasAdAttribution(attribution) ? { attribution } : {};
}

function readCookie(name: string): string | undefined {
  const cookie = typeof document === "undefined" ? undefined : document.cookie;
  if (typeof cookie !== "string") return undefined;
  const prefix = `${name}=`;
  const entry = cookie.split("; ").find((value) => value.startsWith(prefix));
  return entry ? decodeURIComponent(entry.slice(prefix.length)) : undefined;
}
