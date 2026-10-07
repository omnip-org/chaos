import type { CheckoutAttribution } from "../types.js";
import { readBrowserCookie } from "./meta-attribution.js";

/**
 * Ad-platform attribution captured by the checkout call for the later
 * server-side Meta CAPI `Purchase`.
 *
 * `_fbp` is set by Meta's Pixel; `_fbc` is Meta's click cookie, maintained from
 * a landing `fbclid` by `meta-attribution.ts` even when Pixel is disabled. Both
 * are plain, non-HttpOnly cookies by Meta's design. `source_url` is the current
 * page URL — chaos-rust forwards it as the CAPI `event_source_url`.
 *
 * @internal
 */
export function defaultAdAttribution(): CheckoutAttribution {
  const fbc = readBrowserCookie(document, "_fbc");
  const fbp = readBrowserCookie(document, "_fbp");
  const sourceUrl = window.location.href;
  return {
    ...(sourceUrl && { source_url: sourceUrl }),
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
