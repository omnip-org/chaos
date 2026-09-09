import type { CheckoutAttribution } from "../types.js";
import { lastTouchUtmTags, type UtmKey } from "./utm.js";

type UtmStorage = Pick<Storage, "getItem" | "setItem"> | null;

/**
 * Ad-platform attribution for the checkout call (Meta CAPI `InitiateCheckout`
 * / `Purchase`) and cart-line additions (Meta CAPI `AddToCart`).
 *
 * `_fbp` is set by Meta's Pixel; `_fbc` is chaos-js's own copy of the
 * `fbclid` URL param (see `events/browser.ts`'s `maintainFbcCookie`). Both are
 * plain, non-HttpOnly cookies by Meta's design, so reading them needs no extra
 * wiring. `source_url` is the current page URL — chaos-rust forwards it as the
 * CAPI `event_source_url`.
 *
 * `utm` is the *last touch* (journey entry point), read from `storage` so an
 * MPA navigation that dropped `utm_*` from the URL does not lose it — the live
 * URL is only the fallback. This is what reaches the server-side Purchase
 * event via `carts.attribution`.
 * @internal
 */
export function defaultAdAttribution(
  storage: UtmStorage,
): CheckoutAttribution {
  const fbc = readCookie("_fbc");
  const fbp = readCookie("_fbp");
  const sourceUrl =
    typeof window === "undefined" ? undefined : window.location.href;
  const utm = lastTouchUtmTags(storage) as
    | Partial<Record<UtmKey, string>>
    | undefined;
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
export function adAttributionBody(
  storage: UtmStorage,
):
  | { attribution: CheckoutAttribution }
  | Record<string, never> {
  const attribution = defaultAdAttribution(storage);
  return hasAdAttribution(attribution) ? { attribution } : {};
}

function readCookie(name: string): string | undefined {
  const cookie = typeof document === "undefined" ? undefined : document.cookie;
  if (typeof cookie !== "string") return undefined;
  const prefix = `${name}=`;
  const entry = cookie.split("; ").find((value) => value.startsWith(prefix));
  return entry ? decodeURIComponent(entry.slice(prefix.length)) : undefined;
}
