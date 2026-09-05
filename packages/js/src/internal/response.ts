import { ChaosApiError } from "../errors.js";

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function requireData<T>(value: unknown, code: string): T {
  if (!isRecord(value) || !("data" in value) || value.data === null) {
    throw new ChaosApiError(502, code, "storefront response is invalid");
  }
  return value.data as T;
}
