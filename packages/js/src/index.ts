// Browser: one class, `new` it directly and dot into cart/catalog/checkout/
// orders. Event delivery (Pixel/GA4) is wired up internally from
// `StorefrontBrowserOptions.events` — there is no separate analytics class
// to construct or export.
export { StorefrontBrowserClient } from "./ssr/browser.js";
export type { StorefrontBrowserOptions, StorefrontEventsOptions } from "./ssr/browser.js";

// Server: one class, `new` it directly and dot into cart/checkout/orders/
// reviews (cookie- and event-aware) plus catalog/payments/shopperSession
// (pass-through). Meta CAPI (`events` below) is likewise wired up internally
// by the class the caller constructs from `@omnip-org/chaos-js/meta-capi` —
// see that subpath's `ChaosServerEvents`.
export { StorefrontServerClient } from "./ssr/server.js";
export type {
  AddCartLineInput,
  CommerceEventContext,
  ServerClientOptions,
  ServerEventsPort,
  StorefrontCookieJar,
  StorefrontCookieOptions,
  StorefrontSession,
  StorefrontSessionOptions,
  UpdateCartLineInput,
} from "./ssr/server.js";

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
