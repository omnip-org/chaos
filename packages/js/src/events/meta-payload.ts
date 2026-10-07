import { toMajorUnits } from "../money.js";
import type {
  AddToCartAnalyticsInput,
  AnalyticsCommerceItem,
  InitiateCheckoutAnalyticsInput,
  PurchaseAnalyticsInput,
} from "./types.js";

/**
 * Meta's commerce event `custom_data` shape, shared by Pixel's `track()`
 * third argument and Meta's server-side Conversions API — the two are
 * wire-identical, which is what lets chaos-rust's own Purchase CAPI call
 * (sent at payment confirmation) reuse the exact fields this SDK's Pixel
 * calls already send.
 * @internal
 */
export interface MetaCommerceEventData {
  [key: string]: unknown;
  value: number;
  currency: string;
  content_ids: string[];
  content_type: "product";
  contents: Array<{ id: string; quantity: number; item_price: number }>;
  num_items: number;
}

export interface MetaViewContentEventData {
  [key: string]: unknown;
  content_ids: string[];
  content_type: "product";
}

interface CommerceEventInput {
  valueMinor: number;
  currency: string;
  items: AnalyticsCommerceItem[];
}

function toContents(
  items: AnalyticsCommerceItem[],
  currency: string,
): MetaCommerceEventData["contents"] {
  return items.map((item) => ({
    id: item.productId,
    quantity: item.quantity,
    item_price: toMajorUnits(item.priceMinor, currency),
  }));
}

/** @internal */
export function addToCartEventData(
  input: AddToCartAnalyticsInput,
): MetaCommerceEventData {
  return commerceEventData({
    valueMinor: input.valueMinor,
    currency: input.currency,
    items: [
      {
        productId: input.productId,
        productVariantId: input.productVariantId,
        ...(input.itemName !== undefined ? { itemName: input.itemName } : {}),
        ...(input.itemVariant !== undefined
          ? { itemVariant: input.itemVariant }
          : {}),
        quantity: input.quantity,
        priceMinor: input.priceMinor,
      },
    ],
  });
}

/** @internal */
export function initiateCheckoutEventData(
  input: InitiateCheckoutAnalyticsInput,
): MetaCommerceEventData {
  return commerceEventData(input);
}

/** @internal */
export function purchaseEventData(
  input: PurchaseAnalyticsInput,
): MetaCommerceEventData {
  return { ...commerceEventData(input), order_id: input.orderId };
}

/** @internal */
export function viewContentEventData(
  productId: string,
): MetaViewContentEventData {
  return {
    content_ids: [productId],
    content_type: "product",
  };
}

function commerceEventData(
  input: CommerceEventInput,
): MetaCommerceEventData {
  const currency = input.currency.toUpperCase();
  const contents = toContents(input.items, currency);
  return {
    value: toMajorUnits(input.valueMinor, currency),
    currency,
    content_ids: [...new Set(contents.map((content) => content.id))],
    content_type: "product",
    contents,
    num_items: input.items.reduce((total, item) => total + item.quantity, 0),
  };
}
