export const MAX_META_BROWSER_ID_LENGTH = 2_048;

export function isValidMetaBrowserId(value: string | undefined): value is string {
  if (!value || value.length > MAX_META_BROWSER_ID_LENGTH) return false;
  const match = /^fb\.\d+\.(\d{13})\.[^\s]+$/.exec(value);
  return match !== null && Number.isSafeInteger(Number(match[1]));
}

export function compact<T extends Record<string, unknown>>(
  value: T,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  );
}
