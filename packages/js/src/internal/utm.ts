/**
 * `utm_*` campaign tag capture for the attribution chain.
 *
 * A single-page storefront could read `window.location` at every SDK call and
 * be done. An MPA storefront cannot: the visitor lands on `/?utm_source=x`,
 * navigates (full page load, `utm_*` gone from the URL), and only then adds to
 * cart. The SDK therefore persists the first store-scoped UTM snapshot until
 * the lazy shopper-session creation can forward it to
 * `shoppers.attribution.first_seen`. GA4 owns session campaign attribution;
 * checkout does not replay this snapshot to GA4 or Meta.
 *
 * `recordFirstTouchUtm` runs once per client construction. Its read helper
 * falls back to the live URL when storage is unavailable.
 */

import type { BrowserStorage } from "./browser-storage.js";

export const UTM_KEYS = [
  "source",
  "medium",
  "campaign",
  "term",
  "content",
] as const;

export type UtmKey = (typeof UTM_KEYS)[number];
export type UtmTags = Partial<Record<UtmKey, string>>;

/** `utm_*` tags on the current page URL, or `undefined` when there are none
 * (or no browser URL to read). */
export function readUtmTags(): UtmTags | undefined {
  if (typeof window === "undefined") return undefined;
  let params: URLSearchParams;
  try {
    params = new URL(window.location.href).searchParams;
  } catch {
    return undefined;
  }
  const utm: UtmTags = {};
  for (const key of UTM_KEYS) {
    const value = params.get(`utm_${key}`);
    if (value) utm[key] = value;
  }
  return Object.keys(utm).length > 0 ? utm : undefined;
}

type UtmStorage = BrowserStorage | null;

function readStored(storage: UtmStorage, key: string): UtmTags | undefined {
  try {
    const raw = storage?.getItem(key);
    if (!raw) return undefined;
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return undefined;
    }
    const tags: UtmTags = {};
    for (const key of UTM_KEYS) {
      const value = (parsed as Record<string, unknown>)[key];
      if (typeof value === "string" && value) tags[key] = value;
    }
    return Object.keys(tags).length > 0 ? tags : undefined;
  } catch {
    return undefined;
  }
}

function writeStored(
  storage: UtmStorage,
  key: string,
  value: UtmTags,
): void {
  try {
    storage?.setItem(key, JSON.stringify(value));
  } catch {
    // Storage is optional; the snapshot just does not survive this navigation.
  }
}

/**
 * Records the current page's `utm_*` once when no first touch exists yet.
 * A page load with no `utm_*` changes nothing. Call once per page load.
 */
export function recordFirstTouchUtm(
  storage: UtmStorage,
  key: string,
): void {
  const current = readUtmTags();
  if (!current) return;
  if (!readStored(storage, key)) writeStored(storage, key, current);
}

/**
 * UTM tags for shopper-session creation — the visitor's first touch. Falls
 * back to the live URL when nothing is persisted yet (same-page landing
 * before `recordPageUtm` has a prior load to draw on).
 */
export function firstTouchUtmTags(
  storage: UtmStorage,
  key: string,
): UtmTags | undefined {
  return readStored(storage, key) ?? readUtmTags();
}
