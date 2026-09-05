import { toMajorUnits } from "../money.js";
import type {
  AddToCartAnalyticsInput,
  AnalyticsCommerceItem,
  InitiateCheckoutAnalyticsInput,
  PurchaseAnalyticsInput,
} from "./types.js";

/**
 * Meta's shared commerce event shape (`custom_data` on Meta CAPI, the third
 * `track()` argument on Meta Pixel — the two are wire-identical). Built here
 * once so `events/browser.ts` (Pixel) and `events/capi.ts` (CAPI) always
 * project the same fields for the same event, instead of each maintaining
 * its own near-duplicate mapping.
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
    id: item.productVariantId,
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
  return commerceEventData(input);
}

function commerceEventData(
  input: CommerceEventInput,
): MetaCommerceEventData {
  const currency = input.currency.toUpperCase();
  const contents = toContents(input.items, currency);
  return {
    value: toMajorUnits(input.valueMinor, currency),
    currency,
    content_ids: contents.map((content) => content.id),
    content_type: "product",
    contents,
    num_items: input.items.reduce((total, item) => total + item.quantity, 0),
  };
}
