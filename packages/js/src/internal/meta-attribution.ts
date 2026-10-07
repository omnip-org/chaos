import { fnv1a32 } from "./hash.js";
import {
  isValidMetaBrowserId,
  MAX_META_BROWSER_ID_LENGTH,
} from "./meta.js";

const META_FBC_MAX_AGE_SECONDS = 90 * 24 * 60 * 60;
const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f]/;

type SessionStorage = Pick<Storage, "getItem" | "setItem">;

/** Keeps Meta's click id available for the later checkout attribution payload. */
export function maintainMetaFbcCookie(
  namespaceSource: string,
  documentRef: Document | undefined,
  sessionStorage: SessionStorage | null | undefined,
  now: () => number,
): void {
  if (!documentRef) return;
  const fbclid = boundedText(
    new URLSearchParams(documentRef.location?.search ?? "").get("fbclid") ??
      undefined,
    MAX_META_BROWSER_ID_LENGTH,
  );
  if (!fbclid || /\s/.test(fbclid)) return;

  const storageKey =
    `chaos.analytics.${fnv1a32(namespaceSource).toString(36)}.meta.fbc.v2`;
  const stored = readStoredJson(sessionStorage, storageKey);
  const fbc = isStoredFbc(stored, fbclid)
    ? stored.fbc
    : `fb.1.${Math.floor(now())}.${fbclid}`;
  if (!isValidMetaBrowserId(fbc)) return;

  if (!isStoredFbc(stored, fbclid)) {
    writeStoredJson(sessionStorage, storageKey, { fbclid, fbc });
  }
  if (readBrowserCookie(documentRef, "_fbc") !== fbc) {
    writeCookie(documentRef, "_fbc", fbc);
  }
}

/** Reads a browser cookie without allowing malformed percent encoding to fail checkout. */
export function readBrowserCookie(
  documentRef: Pick<Document, "cookie"> | undefined,
  name: string,
): string | undefined {
  const cookie = documentRef?.cookie;
  if (typeof cookie !== "string") return undefined;
  const prefix = `${name}=`;
  const entry = cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(prefix));
  const raw = entry?.slice(prefix.length);
  if (!raw) return undefined;
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
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

function readStoredJson(
  storage: SessionStorage | null | undefined,
  key: string,
): unknown {
  try {
    const value = storage?.getItem(key);
    return value ? JSON.parse(value) : undefined;
  } catch {
    return undefined;
  }
}

function writeStoredJson(
  storage: SessionStorage | null | undefined,
  key: string,
  value: unknown,
): void {
  try {
    storage?.setItem(key, JSON.stringify(value));
  } catch {
    // Session storage is optional.
  }
}
