// One class, `new` it directly and dot into cart/catalog/checkout/orders/
// reviews/shopperSession. Event delivery (Pixel/GA4) is wired up internally
// from `ClientOptions.events` — there is no separate analytics class to
// construct or export. Two projections need an explicit call because the SDK
// can't infer them on its own: `chaos.recordConfirmedPurchase` (also
// reachable as `chaos.orders.recordConfirmedPurchase`) projects a confirmed,
// paid order after `orders.lookupOrder`; `chaos.recordViewContent` re-fires
// ViewContent with the shopper's chosen `productVariantId` once they pick one
// on a product page — `catalog.getProduct` already calls it once
// automatically, but only knows the product's first variant.
export { ChaosStorefrontClient } from "./client.js";
export type { ClientOptions, RequestOptions, StorefrontEventsOptions } from "./client.js";

export type { ViewContentAnalyticsInput } from "./events/types.js";

export { ChaosApiError } from "./errors.js";

export { resolveProductMedia } from "./media.js";

export {
  currencyExponent,
  displayPrice,
  formatPrice,
  toMajorUnits,
  toMinorUnits,
} from "./money.js";
export type { DisplayPrice } from "./money.js";

export {
  getAverageRating,
  getOrderConfirmationState,
  getProductAvailability,
  isVariantAvailable,
  resolveVariant,
  selectedOptionLabel,
} from "./domain.js";
export type {
  SelectedOptions,
  VariantSelectionOption,
  VariantSelectionValue,
  VariantSelectionVariant,
} from "./domain.js";

export * from "./types.js";
