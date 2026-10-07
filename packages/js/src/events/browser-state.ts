import { fnv1a32 } from "../internal/hash.js";
import {
  isValidMetaBrowserId,
  MAX_META_BROWSER_ID_LENGTH,
} from "../internal/meta.js";

const META_FBC_MAX_AGE_SECONDS = 90 * 24 * 60 * 60;
const PROVIDER_EVENT_MAX_AGE_MS = 90 * 24 * 60 * 60 * 1000;
const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f]/;

/** Owns analytics deduplication and Meta click identity stored by the browser. */
export class BrowserEventState {
  private readonly metaFbcStorageKey: string;
  private readonly providerEventStoragePrefix: string;

  constructor(
    publishableKey: string,
    private readonly documentRef: Document,
    private readonly storage: Storage | undefined,
    private readonly sessionStorage: Storage | undefined,
    private readonly now: () => number,
  ) {
    const namespace = fnv1a32(publishableKey).toString(36);
    this.metaFbcStorageKey = `chaos.analytics.${namespace}.meta.fbc.v2`;
    this.providerEventStoragePrefix =
      `chaos.analytics.${namespace}.provider_event.v1.`;
    this.pruneExpiredEvents();
    this.maintainFbcCookie();
  }

  recordOnce(
    eventName: string,
    eventId: string,
    project: () => void,
  ): string | null {
    const storageKey = `${this.providerEventStoragePrefix}${eventName}.${eventId}`;
    try {
      if (this.storage?.getItem(storageKey)) return null;
    } catch {
      // Delivery can proceed without browser storage.
    }
    try {
      project();
    } catch {
      // A later call can retry the same event id.
    }
    try {
      this.storage?.setItem(storageKey, new Date(this.now()).toISOString());
    } catch {
      // Delivery already ran.
    }
    return eventId;
  }

  recordProviderOnce(
    provider: "meta" | "ga4",
    eventName: string,
    eventId: string,
    project: () => boolean,
  ): boolean {
    const storageKey =
      `${this.providerEventStoragePrefix}${provider}.${eventName}.${eventId}`;
    try {
      if (this.storage?.getItem(storageKey)) return false;
    } catch {
      // Delivery can proceed without browser storage.
    }
    if (!project()) return false;
    try {
      this.storage?.setItem(storageKey, new Date(this.now()).toISOString());
    } catch {
      // Delivery already succeeded.
    }
    return true;
  }

  maintainFbcCookie(): void {
    const fbclid = boundedText(
      new URLSearchParams(this.documentRef.location?.search ?? "").get(
        "fbclid",
      ) ?? undefined,
      MAX_META_BROWSER_ID_LENGTH,
    );
    if (!fbclid) return;
    const current = readCookie(this.documentRef, "_fbc");
    const next = this.resolveFbc(fbclid);
    if (next && next !== current) writeCookie(this.documentRef, "_fbc", next);
  }

  private resolveFbc(fbclid: string): string | undefined {
    if (/\s/.test(fbclid)) return undefined;
    const stored = readStoredJson(this.sessionStorage, this.metaFbcStorageKey);
    if (isStoredFbc(stored, fbclid)) return stored.fbc;

    const fbc = `fb.1.${Math.floor(this.now())}.${fbclid}`;
    if (!isValidMetaBrowserId(fbc)) return undefined;
    writeStoredJson(this.sessionStorage, this.metaFbcStorageKey, {
      fbclid,
      fbc,
    });
    return fbc;
  }

  private pruneExpiredEvents(): void {
    if (!this.storage) return;
    try {
      const cutoff = this.now() - PROVIDER_EVENT_MAX_AGE_MS;
      const staleKeys: string[] = [];
      for (let index = 0; index < this.storage.length; index += 1) {
        const key = this.storage.key(index);
        if (!key?.startsWith(this.providerEventStoragePrefix)) continue;
        const recordedAt = Date.parse(this.storage.getItem(key) ?? "");
        if (!Number.isNaN(recordedAt) && recordedAt < cutoff) {
          staleKeys.push(key);
        }
      }
      for (const key of staleKeys) this.storage.removeItem(key);
    } catch {
      // Storage enumeration is best effort.
    }
  }
}

interface StoredFbc {
  fbclid: string;
  fbc: string;
}

function isStoredFbc(value: unknown, fbclid: string): value is StoredFbc {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  return (
    candidate.fbclid === fbclid &&
    typeof candidate.fbc === "string" &&
    isValidMetaBrowserId(candidate.fbc)
  );
}

export function observeHistory(
  windowRef: Window & typeof globalThis,
  listener: () => void,
): () => void {
  const history = windowRef.history;
  if (!history?.pushState || !history?.replaceState) return () => {};
  let state = historyObservers.get(history);
  if (!state) {
    const pushState = history.pushState.bind(history);
    const replaceState = history.replaceState.bind(history);
    const listeners = new Set<() => void>();
    const notify = () => {
      for (const registeredListener of [...listeners]) registeredListener();
    };
    const pushWrapper: History["pushState"] = (...args) => {
      pushState(...args);
      notify();
    };
    const replaceWrapper: History["replaceState"] = (...args) => {
      replaceState(...args);
      notify();
    };
    state = { listeners, pushState, replaceState, pushWrapper, replaceWrapper };
    historyObservers.set(history, state);
    history.pushState = pushWrapper;
    history.replaceState = replaceWrapper;
  }
  state.listeners.add(listener);
  return () => {
    const current = historyObservers.get(history);
    if (!current) return;
    current.listeners.delete(listener);
    if (current.listeners.size > 0) return;
    if (history.pushState === current.pushWrapper) {
      history.pushState = current.pushState;
    }
    if (history.replaceState === current.replaceWrapper) {
      history.replaceState = current.replaceState;
    }
    historyObservers.delete(history);
  };
}

interface HistoryObserverState {
  listeners: Set<() => void>;
  pushState: History["pushState"];
  replaceState: History["replaceState"];
  pushWrapper: History["pushState"];
  replaceWrapper: History["replaceState"];
}

const historyObservers = new WeakMap<History, HistoryObserverState>();

function boundedText(
  value: string | undefined,
  maximumLength: number,
): string | undefined {
  return typeof value === "string" &&
    value.length >= 1 &&
    value.length <= maximumLength &&
    !CONTROL_CHARACTERS.test(value)
    ? value
    : undefined;
}

function readCookie(documentRef: Document, name: string): string | undefined {
  const cookie = documentRef.cookie;
  if (typeof cookie !== "string") return undefined;
  const value = cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(`${name}=`));
  const raw = value?.slice(name.length + 1);
  if (!raw) return undefined;
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
}

function writeCookie(documentRef: Document, name: string, value: string): void {
  try {
    const secure =
      documentRef.location?.protocol === "https:" ? "; Secure" : "";
    documentRef.cookie =
      `${name}=${encodeURIComponent(value)}; Max-Age=${META_FBC_MAX_AGE_SECONDS}; ` +
      `Path=/; SameSite=Lax${secure}`;
  } catch {
    // Cookie storage is optional.
  }
}

function readStoredJson(storage: Storage | undefined, key: string): unknown {
  try {
    const value = storage?.getItem(key);
    return value ? JSON.parse(value) : undefined;
  } catch {
    return undefined;
  }
}

function writeStoredJson(
  storage: Storage | undefined,
  key: string,
  value: unknown,
): void {
  try {
    storage?.setItem(key, JSON.stringify(value));
  } catch {
    // Session storage is optional.
  }
}
