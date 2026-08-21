export { ChaosStorefrontClient, createStorefrontClient } from "./client.js";
export type { ClientOptions, RequestOptions } from "./client.js";

export { ChaosStorefrontAnalytics, createStorefrontAnalytics } from "./analytics.js";
export type { AnalyticsConsentInput, AnalyticsOptions, PageViewInput } from "./analytics.js";

export { ChaosApiError } from "./errors.js";

export { CartResource } from "./resources/cart.js";
export { CatalogResource } from "./resources/catalog.js";
export { CheckoutResource } from "./resources/checkout.js";
export { OrdersResource } from "./resources/orders.js";
export { PaymentsResource } from "./resources/payments.js";
export { ShopperSessionResource } from "./resources/shopper-session.js";

export * from "./types.js";
