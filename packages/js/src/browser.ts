import { ChaosApiError, throwForResponse } from "./errors.js";
import type { ChaosStorefrontAnalytics } from "./analytics.js";
import type {
  CartLineMutation,
  CheckoutAttempt,
  DataEnvelope,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutOptions,
  OrderConfirmationState,
  OrderStatus,
  PurchaseAnalyticsInput,
  TrackedOrder,
} from "./types.js";
import { getOrderConfirmationState } from "./domain.js";

export interface StorefrontBrowserOptions {
  /** Same-origin storefront adapter prefix. Defaults to the shared route prefix. */
  baseUrl?: string;
  fetch?: typeof fetch;
  analytics?: ChaosStorefrontAnalytics;
}

export class StorefrontBrowserClient {
  readonly cart: BrowserCartResource;
  readonly checkout: BrowserCheckoutResource;
  readonly orders: BrowserOrderResource;

  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly analytics: ChaosStorefrontAnalytics | undefined;

  constructor(options: StorefrontBrowserOptions = {}) {
    this.baseUrl = (options.baseUrl ?? "/api").replace(/\/+$/, "");
    this.fetchImpl = options.fetch ?? globalThis.fetch?.bind(globalThis);
    this.analytics = options.analytics;
    if (!this.fetchImpl) throw new TypeError("fetch is required");
    this.cart = new BrowserCartResource(this);
    this.checkout = new BrowserCheckoutResource(this);
    this.orders = new BrowserOrderResource(this);
  }

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

  recordCartMutation(mutation: CartLineMutation): void {
    try {
      this.analytics?.recordCartMutation(mutation);
    } catch {
      // The cart mutation already succeeded; analytics must remain best-effort.
    }
  }

  recordCheckoutCreation(creation: EmbeddedCheckoutCreation): void {
    try {
      this.analytics?.recordCheckoutCreation(creation);
    } catch {
      // The checkout already exists; analytics must remain best-effort.
    }
  }

  recordPurchase(input: PurchaseAnalyticsInput): void {
    try {
      this.analytics?.purchase(input);
    } catch {
      // The order is already confirmed; analytics must remain best-effort.
    }
  }
}

export class BrowserCartResource {
  constructor(private readonly client: StorefrontBrowserClient) {}

  async addLine(variantId: string, quantity = 1): Promise<CartLineMutation> {
    if (!variantId.trim()) throw new TypeError("variantId is required");
    if (!Number.isSafeInteger(quantity) || quantity < 1) {
      throw new RangeError("quantity must be a positive safe integer");
    }
    const body = new URLSearchParams({
      variant_id: variantId,
      quantity: String(quantity),
    });
    const response = await this.client.request<DataEnvelope<CartLineMutation>>(
      "/cart/line-items",
      { method: "POST", body },
    );
    const mutation = requireData<CartLineMutation>(
      response,
      "invalid_cart_mutation_response",
    );
    if (!isCartLineMutation(mutation)) {
      throw new ChaosApiError(
        502,
        "invalid_cart_mutation_response",
        "cart mutation response is invalid",
      );
    }
    this.client.recordCartMutation(mutation);
    return mutation;
  }

  updateLine(variantId: string, quantity: number): Promise<CartLineMutation> {
    if (!variantId.trim()) throw new TypeError("variantId is required");
    if (!Number.isSafeInteger(quantity) || quantity < 1) {
      throw new RangeError("quantity must be a positive safe integer");
    }
    return this.mutateLine(variantId, { quantity: String(quantity) });
  }

  removeLine(variantId: string): Promise<CartLineMutation> {
    if (!variantId.trim()) throw new TypeError("variantId is required");
    return this.mutateLine(variantId, { intent: "remove" });
  }

  private async mutateLine(
    variantId: string,
    values: Record<string, string>,
  ): Promise<CartLineMutation> {
    const body = new URLSearchParams(values);
    const response = await this.client.request<DataEnvelope<CartLineMutation>>(
      `/cart/line-items/${encodeURIComponent(variantId)}`,
      { method: "POST", body },
    );
    const mutation = requireData<CartLineMutation>(
      response,
      "invalid_cart_mutation_response",
    );
    if (!isCartLineMutation(mutation)) {
      throw new ChaosApiError(
        502,
        "invalid_cart_mutation_response",
        "cart mutation response is invalid",
      );
    }
    this.client.recordCartMutation(mutation);
    return mutation;
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

  async resumeEmbeddedCheckout(
    checkoutAttemptId: string,
  ): Promise<EmbeddedCheckoutCreation> {
    if (!checkoutAttemptId.trim()) {
      throw new TypeError("checkoutAttemptId is required");
    }
    const response = await this.client.request<DataEnvelope<EmbeddedCheckoutCreation>>(
      "/checkout/resume",
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ checkoutAttemptId }),
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
    return creation;
  }

  async listCheckoutAttempts(): Promise<CheckoutAttempt[]> {
    const response = await this.client.request<DataEnvelope<CheckoutAttempt[]>>(
      "/checkout-attempts",
      { method: "GET" },
    );
    const attempts = requireData<CheckoutAttempt[]>(
      response,
      "invalid_checkout_attempts_response",
    );
    if (
      !Array.isArray(attempts) ||
      !attempts.every((attempt) => isCheckoutAttempt(attempt))
    ) {
      throw new ChaosApiError(
        502,
        "invalid_checkout_attempts_response",
        "checkout attempts response is invalid",
      );
    }
    return attempts;
  }
}

export class BrowserOrderResource {
  constructor(private readonly client: StorefrontBrowserClient) {}

  recordPurchase(input: PurchaseAnalyticsInput): void {
    this.client.recordPurchase(input);
  }

  async getStatus(orderId: string): Promise<OrderConfirmationState> {
    if (!orderId.trim()) throw new TypeError("orderId is required");
    const response = await this.client.request<DataEnvelope<OrderStatus>>(
      `/orders/${encodeURIComponent(orderId)}/status`,
      { cache: "no-store" },
    );
    const status = requireData<OrderStatus>(response, "invalid_order_status_response");
    if (
      !isRecord(status) ||
      typeof status.status !== "string" ||
      typeof status.payment_status !== "string"
    ) {
      throw new ChaosApiError(
        502,
        "invalid_order_status_response",
        "order status response is invalid",
      );
    }
    return getOrderConfirmationState(
      status.status,
      status.payment_status,
    );
  }

  async getTrackedOrder(trackingToken: string): Promise<TrackedOrder> {
    if (!/^ot_[^\s]{1,509}$/.test(trackingToken)) {
      throw new ChaosApiError(400, "invalid_tracking_token", "tracking token is invalid");
    }
    const response = await this.client.request<DataEnvelope<TrackedOrder>>(
      "/orders/tracking",
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ tracking_token: trackingToken }),
        cache: "no-store",
      },
    );
    return requireData<TrackedOrder>(response, "invalid_tracked_order_response");
  }
}

export function createStorefrontBrowserClient(
  options: StorefrontBrowserOptions = {},
): StorefrontBrowserClient {
  return new StorefrontBrowserClient(options);
}

function requireData<T>(value: unknown, code: string): T {
  if (!isRecord(value) || !("data" in value) || value.data === null) {
    throw new ChaosApiError(502, code, "storefront response is invalid");
  }
  return value.data as T;
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

function isEmbeddedCheckoutCreation(
  value: unknown,
): value is EmbeddedCheckoutCreation {
  if (!isRecord(value) || !isRecord(value.checkout) || !isRecord(value.cart)) {
    return false;
  }
  const checkout = value.checkout;
  return (
    typeof checkout.checkout_attempt_id === "string" &&
    typeof checkout.order_id === "string" &&
    typeof checkout.source_cart_id === "string" &&
    typeof checkout.successor_cart_id === "string" &&
    isCheckoutAttemptStatus(checkout.status) &&
    typeof checkout.expires_at === "string" &&
    isRecord(checkout.client_action) &&
    checkout.client_action.type === "mount_embedded_checkout" &&
    typeof checkout.client_action.public_key === "string" &&
    typeof checkout.client_action.client_token === "string" &&
    isCart(value.cart)
  );
}

function isCheckoutAttempt(value: unknown): value is CheckoutAttempt {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.order_id === "string" &&
    typeof value.source_cart_id === "string" &&
    typeof value.successor_cart_id === "string" &&
    isCheckoutAttemptStatus(value.status) &&
    typeof value.expires_at === "string" &&
    typeof value.created_at === "string" &&
    typeof value.updated_at === "string"
  );
}

function isCheckoutAttemptStatus(value: unknown): boolean {
  return (
    value === "creating" ||
    value === "open" ||
    value === "paid" ||
    value === "failed" ||
    value === "cancelled" ||
    value === "expired"
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
