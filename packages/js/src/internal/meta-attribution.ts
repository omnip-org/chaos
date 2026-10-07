import {
  isValidMetaBrowserId,
  MAX_META_BROWSER_ID_LENGTH,
} from "./meta.js";

const META_FBC_MAX_AGE_SECONDS = 90 * 24 * 60 * 60;
const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f]/;

/** Keeps Meta's click id available for the later checkout attribution payload. */
export function maintainMetaFbcCookie(
  documentRef: Document | undefined,
  now: () => number,
): void {
  if (!documentRef) return;
  const fbclid = boundedText(
    new URLSearchParams(documentRef.location?.search ?? "").get("fbclid") ??
      undefined,
    MAX_META_BROWSER_ID_LENGTH,
  );
  if (!fbclid || /\s/.test(fbclid)) return;

  const existing = readBrowserCookie(documentRef, "_fbc");
  if (isFbcForClick(existing, fbclid)) return;

  const fbc = `fb.1.${Math.floor(now())}.${fbclid}`;
  if (!isValidMetaBrowserId(fbc)) return;

  writeCookie(documentRef, "_fbc", fbc);
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

function isFbcForClick(value: string | undefined, fbclid: string): boolean {
  return isValidMetaBrowserId(value) && value.endsWith(`.${fbclid}`);
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
