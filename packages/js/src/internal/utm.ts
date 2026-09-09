/**
 * `utm_*` campaign tag capture for the attribution chain.
 *
 * A single-page storefront could read `window.location` at every SDK call and
 * be done. An MPA storefront cannot: the visitor lands on `/?utm_source=x`,
 * navigates (full page load, `utm_*` gone from the URL), and only then adds to
 * cart or checks out. So the SDK persists two snapshots in `storage`:
 *
 * - **first touch** (`utm_first_touch`) — written once, on the first page load
 *   that carries `utm_*`, never overwritten. The visitor's acquisition source.
 *   Forwarded to shopper-session creation, which the server records as
 *   `shoppers.attribution.first_seen`.
 * - **last touch** (`utm_last_touch`) — overwritten on every page load that
 *   carries `utm_*`; a load with none leaves it untouched. The entry point of
 *   the visitor's *current* shopping journey. Forwarded to checkout / AddToCart
 *   (`attribution.utm` → Meta CAPI Purchase / AddToCart) and to the
 *   `last_seen` session refresh.
 *
 * `recordPageUtm(storage)` runs once per client construction (i.e. once per
 * page load) and does both writes. The read helpers fall back to the live URL
 * only when `storage` holds nothing — a same-page landing still works with no
 * persisted value.
 */

export const UTM_KEYS = [
  "source",
  "medium",
  "campaign",
  "term",
  "content",
] as const;

export type UtmKey = (typeof UTM_KEYS)[number];

/** `utm_*` tags on the current page URL, or `undefined` when there are none
 * (or no browser URL to read). */
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

function toRequestParams(
  tags: Partial<Record<UtmKey, string>> | undefined,
): UtmRequestParams {
  const params: UtmRequestParams = {};
  if (tags) {
    for (const [key, value] of Object.entries(tags)) {
      if (value) params[`utm_${key as UtmKey}`] = value;
    }
  }
  return params;
}

type UtmStorage = Pick<Storage, "getItem" | "setItem"> | null;

const FIRST_TOUCH_STORAGE_KEY = "chaos.storefront.utm_first_touch";
const LAST_TOUCH_STORAGE_KEY = "chaos.storefront.utm_last_touch";

function readStored(
  storage: UtmStorage,
  key: string,
): UtmRequestParams | undefined {
  try {
    const raw = storage?.getItem(key);
    if (!raw) return undefined;
    const parsed = JSON.parse(raw) as UtmRequestParams;
    return parsed && typeof parsed === "object" ? parsed : undefined;
  } catch {
    return undefined;
  }
}

function writeStored(
  storage: UtmStorage,
  key: string,
  value: UtmRequestParams,
): void {
  try {
    storage?.setItem(key, JSON.stringify(value));
  } catch {
    // Storage is optional; the snapshot just does not survive this navigation.
  }
}

/**
 * Records the current page's `utm_*` into `storage`: first touch if nothing
 * was recorded before, last touch every time. A page load with no `utm_*`
 * changes neither. Call once per page load (client construction).
 */
export function recordPageUtm(storage: UtmStorage): void {
  const current = toRequestParams(readUtmTags());
  if (Object.keys(current).length === 0) return;
  if (!readStored(storage, FIRST_TOUCH_STORAGE_KEY)) {
    writeStored(storage, FIRST_TOUCH_STORAGE_KEY, current);
  }
  writeStored(storage, LAST_TOUCH_STORAGE_KEY, current);
}

/**
 * `utm_*` query params for shopper-session creation — the visitor's first
 * touch. Falls back to the live URL when nothing is persisted yet (same-page
 * landing before `recordPageUtm` has a prior load to draw on). `{}` when
 * there is nothing anywhere.
 */
export function firstTouchUtmParams(storage: UtmStorage): UtmRequestParams {
  return (
    readStored(storage, FIRST_TOUCH_STORAGE_KEY) ??
    toRequestParams(readUtmTags())
  );
}

/**
 * `utm_*` tags for the checkout / AddToCart `attribution.utm` body and the
 * `last_seen` refresh — the entry point of the current shopping journey.
 * Falls back to the live URL when nothing is persisted. `undefined` when
 * there is nothing anywhere, so the attribution body omits `utm` entirely.
 */
export function lastTouchUtmTags(
  storage: UtmStorage,
): Partial<Record<UtmKey, string>> | undefined {
  const stored = readStored(storage, LAST_TOUCH_STORAGE_KEY);
  if (stored) {
    const tags: Partial<Record<UtmKey, string>> = {};
    for (const key of UTM_KEYS) {
      const value = stored[`utm_${key}`];
      if (value) tags[key] = value;
    }
    if (Object.keys(tags).length > 0) return tags;
  }
  return readUtmTags();
}

/** `utm_*` query params form of {@link lastTouchUtmTags}, for the session
 * refresh call. `{}` when there is nothing. */
export function lastTouchUtmParams(storage: UtmStorage): UtmRequestParams {
  return toRequestParams(lastTouchUtmTags(storage));
}
