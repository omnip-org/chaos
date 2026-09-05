import { ChaosStorefrontAnalytics, type AnalyticsOptions } from "../events/browser.js";
import { ChaosApiError, throwForResponse } from "../errors.js";
import { isRecord, requireData } from "../internal/response.js";
import type { GetProductParams, ListProductsParams } from "../resources/catalog.js";
import type { OrderLookupParams } from "../resources/orders.js";
import type {
  CartLineMutation,
  DataEnvelope,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutOptions,
  OrderLookup,
  PageEnvelope,
  Product,
} from "../types.js";

/**
 * Meta Pixel/GA4 event delivery, keyed by destination: pass `metaPixel` to
 * turn on Pixel, `ga4` to turn on GA4, omit either to leave it off — there
 * is no separate enable flag. `document`/`window`/`storage`/`randomUUID`/
 * `now`/`autoStart` exist for test injection; a real storefront never sets
 * them.
 */
export type StorefrontEventsOptions = Omit<AnalyticsOptions, "publishableKey">;

export interface StorefrontBrowserOptions {
  /** Same-origin storefront adapter prefix. Defaults to the shared route prefix. */
  baseUrl?: string;
  fetch?: typeof fetch;
  events?: StorefrontEventsOptions;
}

export class StorefrontBrowserClient {
  readonly cart: BrowserCartResource;
  readonly catalog: BrowserCatalogResource;
  readonly checkout: BrowserCheckoutResource;
  readonly orders: BrowserOrderResource;

  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly analytics: ChaosStorefrontAnalytics;

  constructor(options: StorefrontBrowserOptions = {}) {
    this.baseUrl = (options.baseUrl ?? "/api").replace(/\/+$/, "");
    this.fetchImpl = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (!this.fetchImpl) throw new TypeError("fetch is required");
    // Reuse the route prefix as a stable namespace for browser analytics.
    this.analytics = new ChaosStorefrontAnalytics({
      publishableKey: this.baseUrl,
      ...options.events,
    });
    this.cart = new BrowserCartResource(this);
    this.catalog = new BrowserCatalogResource(this);
    this.checkout = new BrowserCheckoutResource(this);
    this.orders = new BrowserOrderResource(this);
  }

  /** @internal Used by the resource facades. */
  async request<T>(path: string, init: RequestInit = {}): Promise<T> {
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      ...init,
      headers,
      credentials: "same-origin",
    });
    if (!response.ok) await throwForResponse(response);
    try {
      return (await response.json()) as T;
    } catch {
      throw new ChaosApiError(502, "invalid_storefront_response", "storefront response is invalid");
    }
  }

  /** @internal Used by the cart facade. */
  recordCartMutation(mutation: CartLineMutation): void {
    try {
      this.analytics.recordCartMutation(mutation);
    } catch {
      // The cart mutation already succeeded; analytics must remain best-effort.
    }
  }

  /** @internal Used by the checkout facade. */
  recordCheckoutCreation(creation: EmbeddedCheckoutCreation): void {
    try {
      this.analytics.recordCheckoutCreation(creation);
    } catch {
      // The checkout already exists; analytics must remain best-effort.
    }
  }

  /** @internal Used by the order facade. */
  recordConfirmedPurchase(
    order: Pick<
      OrderLookup,
      "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
    >,
  ): void {
    try {
      this.analytics.recordConfirmedPurchase(order);
    } catch {
      // The order is already confirmed; analytics must remain best-effort.
    }
  }

  /** @internal Used by the catalog facade. */
  recordSearch(input: { query: string }): void {
    try {
      this.analytics.search(input);
    } catch {
      // The search already ran; analytics must remain best-effort.
    }
  }

  /** @internal Used by the catalog facade. */
  recordViewContent(input: { productId: string; productVariantId?: string }): void {
    try {
      this.analytics.viewContent(input);
    } catch {
      // The product already loaded; analytics must remain best-effort.
    }
  }
}

export class BrowserCartResource {
  constructor(private readonly client: StorefrontBrowserClient) {}

  async addLine(variantId: string, quantity = 1): Promise<CartLineMutation> {
    requireVariantId(variantId);
    if (!Number.isSafeInteger(quantity) || quantity < 1) {
      throw new RangeError("quantity must be a positive safe integer");
    }
    const response = await this.client.request<DataEnvelope<CartLineMutation>>(
      "/cart/line-items",
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ variant_id: variantId, quantity }),
      },
    );
    const mutation = requireCartLineMutation(response);
    this.client.recordCartMutation(mutation);
    return mutation;
  }

  updateLine(variantId: string, quantity: number): Promise<CartLineMutation> {
    requireVariantId(variantId);
    if (!Number.isSafeInteger(quantity) || quantity < 1) {
      throw new RangeError("quantity must be a positive safe integer");
    }
    return this.mutateLine(variantId, { quantity });
  }

  removeLine(variantId: string): Promise<CartLineMutation> {
    requireVariantId(variantId);
    return this.mutateLine(variantId, { intent: "remove" });
  }

  private async mutateLine(
    variantId: string,
    values: Record<string, unknown>,
  ): Promise<CartLineMutation> {
    const response = await this.client.request<DataEnvelope<CartLineMutation>>(
      `/cart/line-items/${encodeURIComponent(variantId)}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(values),
      },
    );
    const mutation = requireCartLineMutation(response);
    this.client.recordCartMutation(mutation);
    return mutation;
  }
}

/** Forwards catalog reads to the storefront's own same-origin route and records the matching Search/ViewContent event. */
export class BrowserCatalogResource {
  constructor(private readonly client: StorefrontBrowserClient) {}

  async listProducts(params: ListProductsParams = {}): Promise<PageEnvelope<Product>> {
    const search = new URLSearchParams();
    for (const [key, value] of Object.entries(params)) {
      if (value !== undefined) search.set(key, String(value));
    }
    const query = search.toString();
    const response = await this.client.request<PageEnvelope<Product>>(
      `/products${query ? `?${query}` : ""}`,
    );
    if (params.q) this.client.recordSearch({ query: params.q });
    return response;
  }

  async getProduct(handle: string, params: GetProductParams = {}): Promise<DataEnvelope<Product>> {
    const query = params.currency ? `?currency=${encodeURIComponent(params.currency)}` : "";
    const response = await this.client.request<DataEnvelope<Product>>(
      `/products/${encodeURIComponent(handle)}${query}`,
    );
    this.client.recordViewContent({ productId: response.data.id });
    return response;
  }
}

export class BrowserCheckoutResource {
  constructor(private readonly client: StorefrontBrowserClient) {}

  async createEmbeddedCheckout(
    options: EmbeddedCheckoutOptions | string,
  ): Promise<EmbeddedCheckoutCreation> {
    const resolvedOptions =
      typeof options === "string" ? { returnUrl: options } : options;
    if (!resolvedOptions.returnUrl.trim()) {
      throw new TypeError("returnUrl is required");
    }
    const response = await this.client.request<DataEnvelope<EmbeddedCheckoutCreation>>(
      "/checkout",
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          returnUrl: resolvedOptions.returnUrl,
          ...(resolvedOptions.email ? { email: resolvedOptions.email } : {}),
        }),
      },
    );
    const creation = requireData<EmbeddedCheckoutCreation>(
      response,
      "invalid_checkout_response",
    );
    if (!isEmbeddedCheckoutCreation(creation)) {
      throw new ChaosApiError(
        502,
        "invalid_checkout_response",
        "checkout response is invalid",
      );
    }
    this.client.recordCheckoutCreation(creation);
    return creation;
  }
}

export class BrowserOrderResource {
  constructor(private readonly client: StorefrontBrowserClient) {}

  /**
   * Projects a confirmed, paid order to Meta Pixel/GA4 — never inferred from
   * browser activity. Typically called on a return page right after
   * `lookupOrder`. No-op unless the order is confirmed and paid.
   */
  recordConfirmedPurchase(
    order: Pick<
      OrderLookup,
      "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
    >,
  ): void {
    this.client.recordConfirmedPurchase(order);
  }

  async lookupOrder(params: OrderLookupParams): Promise<OrderLookup> {
    const orderNumber = params.orderNumber?.trim() ?? "";
    const email = params.email?.trim() ?? "";
    if (!/^W-[0-9]{8}-[0-9A-HJKMNP-TV-Z]{8}$/.test(orderNumber)) {
      throw new ChaosApiError(400, "invalid_order_number", "order number is invalid");
    }
    if (email.length === 0) {
      throw new ChaosApiError(400, "invalid_email", "email is required");
    }
    const response = await this.client.request<DataEnvelope<OrderLookup>>(
      "/orders/lookup",
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ order_number: orderNumber, email }),
        cache: "no-store",
      },
    );
    return requireData<OrderLookup>(response, "invalid_order_lookup_response");
  }
}

function isCartLineMutation(value: unknown): value is CartLineMutation {
  if (!isRecord(value) || !isRecord(value.cart)) return false;
  return (
    typeof value.product_variant_id === "string" &&
    Number.isSafeInteger(value.previous_quantity) &&
    Number.isSafeInteger(value.new_quantity) &&
    typeof value.removed === "boolean" &&
    isCart(value.cart)
  );
}

function requireCartLineMutation(value: unknown): CartLineMutation {
  const mutation = requireData<CartLineMutation>(
    value,
    "invalid_cart_mutation_response",
  );
  if (!isCartLineMutation(mutation)) {
    throw new ChaosApiError(
      502,
      "invalid_cart_mutation_response",
      "cart mutation response is invalid",
    );
  }
  return mutation;
}

function requireVariantId(variantId: string): void {
  if (!variantId.trim()) throw new TypeError("variantId is required");
}

function isEmbeddedCheckoutCreation(
  value: unknown,
): value is EmbeddedCheckoutCreation {
  if (
    !isRecord(value) ||
    !isRecord(value.checkout) ||
    !isRecord(value.source_cart) ||
    !isRecord(value.cart)
  ) {
    return false;
  }
  const checkout = value.checkout;
  return (
    typeof checkout.order_number === "string" &&
    isRecord(checkout.client_action) &&
    checkout.client_action.type === "mount_embedded_checkout" &&
    typeof checkout.client_action.public_key === "string" &&
    typeof checkout.client_action.client_token === "string" &&
    isCart(value.source_cart) &&
    isCart(value.cart)
  );
}

function isCart(value: Record<string, unknown>): boolean {
  return (
    typeof value.id === "string" &&
    typeof value.currency === "string" &&
    Array.isArray(value.lines) &&
    value.lines.every(
      (line) =>
        isRecord(line) &&
        typeof line.product_id === "string" &&
        typeof line.product_variant_id === "string" &&
        Number.isSafeInteger(line.quantity) &&
        Number.isSafeInteger(line.unit_price_amount_minor),
    )
  );
}
