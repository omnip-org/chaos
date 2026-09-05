/**
 * SDK-invented commerce event input shapes — camelCase, not the Storefront
 * API's snake_case wire contract (see `../types.ts`). These are what a
 * caller builds to hand to `ChaosStorefrontAnalytics` (events/browser.ts),
 * kept separate from `../types.ts` so that file's wire-format contract stays
 * exactly that.
 */

import type { CurrencyCode, UUID } from "../types.js";

/** Canonical item shape shared by every browser/CAPI commerce event. */
export interface AnalyticsCommerceItem {
  productId: UUID;
  productVariantId: UUID;
  quantity: number;
  priceMinor: number;
}

/** Canonical browser projection of a confirmed, paid Order. */
export interface PurchaseAnalyticsInput {
  orderId: UUID;
  valueMinor: number;
  currency: CurrencyCode;
  items: AnalyticsCommerceItem[];
}

export interface AddToCartAnalyticsInput {
  cartId?: UUID;
  productId: UUID;
  productVariantId: UUID;
  quantity: number;
  priceMinor: number;
  valueMinor: number;
  currency: CurrencyCode;
}

export interface InitiateCheckoutAnalyticsInput {
  cartId: UUID;
  orderNumber: string;
  valueMinor: number;
  currency: CurrencyCode;
  items: AnalyticsCommerceItem[];
}
