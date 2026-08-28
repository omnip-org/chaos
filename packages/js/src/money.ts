import type { Price } from "./types.js";

const ZERO_DECIMAL_CURRENCIES = new Set([
  "BIF",
  "CLP",
  "DJF",
  "GNF",
  "JPY",
  "KMF",
  "KRW",
  "MGA",
  "PYG",
  "RWF",
  "UGX",
  "VND",
  "VUV",
  "XAF",
  "XOF",
  "XPF",
]);

const THREE_DECIMAL_CURRENCIES = new Set(["BHD", "JOD", "KWD", "OMR", "TND"]);

export interface DisplayPrice {
  amount: number;
  currencyCode: string;
}

/** Returns the ISO-4217 minor-unit exponent used by Chaos and the providers. */
export function currencyExponent(currency: string): number {
  const normalized = normalizeCurrency(currency);
  if (ZERO_DECIMAL_CURRENCIES.has(normalized)) return 0;
  if (THREE_DECIMAL_CURRENCIES.has(normalized)) return 3;
  return 2;
}

export function toMinorUnits(amount: number, currency: string): number {
  if (!Number.isFinite(amount)) throw new TypeError("amount must be finite");
  const minor = Math.round(amount * 10 ** currencyExponent(currency));
  if (!Number.isSafeInteger(minor)) {
    throw new RangeError("amount is outside the safe integer range");
  }
  return minor;
}

export function toMajorUnits(amountMinor: number, currency: string): number {
  if (!Number.isSafeInteger(amountMinor)) {
    throw new TypeError("amountMinor must be a safe integer");
  }
  return amountMinor / 10 ** currencyExponent(currency);
}

export function displayPrice(price: Price | undefined): DisplayPrice | undefined {
  if (!price) return undefined;
  return {
    amount: toMajorUnits(price.amount_minor, price.currency),
    currencyCode: normalizeCurrency(price.currency),
  };
}

/** Formats a major-unit amount for a customer-facing locale. */
export function formatPrice(
  amount: number,
  currency: string,
  locale = "en-US",
): string {
  return new Intl.NumberFormat(locale, {
    style: "currency",
    currency: normalizeCurrency(currency),
  }).format(amount);
}

function normalizeCurrency(currency: string): string {
  if (!/^[A-Za-z]{3}$/.test(currency)) {
    throw new TypeError("currency must be an ISO 4217 code");
  }
  return currency.toUpperCase();
}
