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
