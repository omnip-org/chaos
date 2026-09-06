/**
 * Reads `utm_*` campaign tags off the current page URL. Best-effort, with
 * no first-touch persistence: whatever the URL carries when the SDK calls
 * are made (shopper-session creation and checkout attribution) is what gets
 * forwarded to the API.
 */

export const UTM_KEYS = [
  "source",
  "medium",
  "campaign",
  "term",
  "content",
] as const;

export type UtmKey = (typeof UTM_KEYS)[number];

/** Short-keyed tags for the checkout `attribution.utm` body, or `undefined`
 * when there are none (or no browser URL to read). */
export function readUtmTags(): Partial<Record<UtmKey, string>> | undefined {
  if (typeof window === "undefined") return undefined;
  let params: URLSearchParams;
  try {
    params = new URL(window.location.href).searchParams;
  } catch {
    return undefined;
  }
  const utm: Partial<Record<UtmKey, string>> = {};
  for (const key of UTM_KEYS) {
    const value = params.get(`utm_${key}`);
    if (value) utm[key] = value;
  }
  return Object.keys(utm).length > 0 ? utm : undefined;
}

export type UtmRequestParams = Partial<Record<`utm_${UtmKey}`, string>>;

/** `utm_*`-prefixed params for a request query string; `{}` when there are
 * none, which the client's URL builder simply omits. */
export function utmRequestParams(): UtmRequestParams {
  const tags = readUtmTags();
  const params: UtmRequestParams = {};
  if (tags) {
    for (const [key, value] of Object.entries(tags)) {
      params[`utm_${key as UtmKey}`] = value;
    }
  }
  return params;
}
