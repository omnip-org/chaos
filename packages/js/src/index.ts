export { ChaosStorefrontClient, createStorefrontClient } from "./client.js";
export type { ClientOptions } from "./client.js";

export {
  BrowserCartResource,
  BrowserCheckoutResource,
  BrowserOrderResource,
  StorefrontBrowserClient,
  createStorefrontBrowserClient,
} from "./ssr/browser.js";
export type { StorefrontBrowserOptions } from "./ssr/browser.js";

export {
  ChaosStorefrontAnalytics,
  createStorefrontAnalytics,
} from "./analytics.js";
export type { AnalyticsOptions, PageViewInput } from "./analytics.js";
export type {
  AddToCartAnalyticsInput,
  AnalyticsCommerceItem,
  InitiateCheckoutAnalyticsInput,
  PurchaseAnalyticsInput,
} from "./analytics-types.js";

export { ChaosApiError } from "./errors.js";

export { resolveProductMedia } from "./media.js";

export { CartResource } from "./resources/cart.js";
export { CatalogResource } from "./resources/catalog.js";
export { OrdersResource } from "./resources/orders.js";
export { PaymentsResource } from "./resources/payments.js";
export { ReviewsResource } from "./resources/reviews.js";
export { ShopperSessionResource } from "./resources/shopper-session.js";

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
  toPurchaseAnalyticsInput,
} from "./domain.js";
export type {
  SelectedOptions,
  VariantSelectionOption,
  VariantSelectionValue,
  VariantSelectionVariant,
} from "./domain.js";

export {
  addCartLine,
  addCartLineFromRequest,
  cartItemCount,
  createProductReviewFromRequest,
  createEmbeddedCheckoutFromRequest,
  createServerStorefrontClient,
  createShopperTokenStorage,
  getOrCreateCartSession,
  lookupOrderFromRequest,
  peekCartSession,
  persistCartSession,
  recordConfirmedPurchaseCapi,
  updateCartLine,
  updateCartLineFromRequest,
  DEFAULT_CART_COOKIE_NAME,
  DEFAULT_SHOPPER_TOKEN_COOKIE_NAME,
} from "./ssr/server.js";
export type {
  AddCartLineInput,
  EmbeddedCheckoutRequestInput,
  ServerClientOptions,
  StorefrontCookieJar,
  StorefrontCookieOptions,
  StorefrontSession,
  StorefrontSessionOptions,
  UpdateCartLineInput,
} from "./ssr/server.js";

export * from "./types.js";
