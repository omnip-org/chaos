import type { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import {
  defaultAdAttribution,
  hasAdAttribution,
} from "../internal/attribution.js";
import { isRecord, requireData } from "../internal/response.js";
import { mountEmbeddedCheckout as mountStripeEmbeddedCheckout } from "../providers/stripe.js";
import type {
  Cart,
  CheckoutOrder,
  CheckoutAttribution,
  DataEnvelope,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutMount,
  EmbeddedCheckoutOptions,
  EmbeddedCheckoutSession,
  EmbeddedCheckoutStart,
  MountEmbeddedCheckoutOptions,
  PaymentClientAction,
  PaymentProvider,
  WaitForCheckoutOrderOptions,
} from "../types.js";

const DEFAULT_POLL_INTERVAL_MS = 1_000;
const DEFAULT_WAIT_TIMEOUT_MS = 30_000;
const CHECKOUT_ORDER_ID_PARAM = "order_id";
const CHECKOUT_TOKEN_PARAM = "checkout_token";

interface EmbeddedCheckoutRequest {
  payment_provider: PaymentProvider;
  attribution?: CheckoutAttribution;
}

interface EmbeddedCheckoutApiSession {
  order_id: string;
  order_number: string;
  checkout_token: string;
  client_action: PaymentClientAction;
}

interface CheckoutAccessResponse {
  order: CheckoutOrder;
  client_action?: PaymentClientAction;
}

interface CheckoutRecovery {
  order: CheckoutOrder;
  checkout?: EmbeddedCheckoutSession;
}

interface CheckoutLink {
  orderId: string;
  checkoutToken: string;
  url: string;
}

export class PaymentsResource {
  /** One idempotency key per Cart for all retries in this browser instance. */
  private readonly idempotencyKeys = new Map<string, string>();
  private activeCheckout: EmbeddedCheckoutSession | undefined;

  constructor(private readonly client: ChaosStorefrontClient) {}

  /**
   * Mounts the checkout most recently created by this client. After a reload
   * or on a shared link, the same method recovers it from the current URL.
   * A terminal recovered Order calls `onComplete` and returns `null`.
   */
  async mountEmbeddedCheckout(
    container: HTMLElement,
    options: MountEmbeddedCheckoutOptions = {},
  ): Promise<EmbeddedCheckoutMount | null> {
    let checkout = this.activeCheckout;
    if (!checkout) {
      const recovered = await this.recoverEmbeddedCheckout();
      checkout = recovered.data.checkout;
      if (!checkout) {
        await options.onComplete?.(recovered.data.order);
        return null;
      }
    }

    activateCheckoutUrl(checkout.checkout_url);
    return mountStripeEmbeddedCheckout(checkout.client_action, container, {
      ...(options.onAnalyticsEvent
        ? { onAnalyticsEvent: options.onAnalyticsEvent }
        : {}),
      onComplete: () => {
        void this.waitForCheckoutOrder(checkout, options.confirmation)
          .then(({ data }) => options.onComplete?.(data))
          .catch((error: unknown) => {
            if (options.onError) {
              options.onError(error);
              return;
            }
            console.error("[chaos-js] checkout confirmation failed", error);
          });
      },
    });
  }

  private async recoverEmbeddedCheckout(
    url = window.location.href,
  ): Promise<DataEnvelope<CheckoutRecovery>> {
    const link = parseCheckoutLink(url);
    if (!link) {
      throw new ChaosApiError(
        400,
        "checkout_link_required",
        "the current URL does not contain a checkout link",
      );
    }
    const access = await this.readCheckout(link);
    const checkout = access.client_action
      ? checkoutSession(link, access.order, access.client_action)
      : undefined;
    if (!checkout && !isTerminalCheckoutOrder(access.order)) {
      throw invalidCheckoutResponse();
    }
    this.activeCheckout = checkout;
    const order = isTerminalCheckoutOrder(access.order)
      ? await this.confirmTerminalOrder(access.order)
      : access.order;
    return {
      data: {
        order,
        ...(checkout ? { checkout } : {}),
      },
    };
  }

  async createEmbeddedCheckout(
    cartId: string,
    options: EmbeddedCheckoutOptions = {},
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    const { cart, checkout } = await this.client.cart.runExclusive(
      cartId,
      async () => {
        const cart = await this.client.cart.snapshot(cartId);
        const checkout = await this.createEmbeddedCheckoutForCart(cart, options);
        return { cart, checkout };
      },
    );
    this.client.recordCheckoutCreation({
      checkout: checkout.data,
      source_cart: cart,
    });
    return checkout;
  }

  /** Starts checkout from a Cart body the caller just loaded. */
  async createEmbeddedCheckoutFromCart(
    cart: Cart,
    options: EmbeddedCheckoutOptions = {},
  ): Promise<DataEnvelope<EmbeddedCheckoutStart>> {
    const checkout = await this.client.cart.runExclusive(cart.id, () =>
      this.createEmbeddedCheckoutForCart(cart, options),
    );
    const start: EmbeddedCheckoutStart = {
      checkout: checkout.data,
      source_cart: cart,
    };
    this.client.recordCheckoutCreation(start);
    return { data: start };
  }

  async createEmbeddedCheckoutWithCart(
    cartId: string,
    options: EmbeddedCheckoutOptions = {},
  ): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
    const { checkout, sourceCart } = await this.client.cart.runExclusive(
      cartId,
      async () => {
        const cart = await this.client.cart.snapshot(cartId);
        const result = await this.createEmbeddedCheckoutForCart(cart, options);
        return { checkout: result, sourceCart: cart };
      },
    );
    const start: EmbeddedCheckoutStart = {
      checkout: checkout.data,
      source_cart: sourceCart,
    };
    // Checkout already exists at this point. Project InitiateCheckout before
    // acquiring the next shopping Cart so a secondary Cart failure cannot
    // erase the successful payment handoff from browser analytics.
    this.client.recordCheckoutCreation(start);
    const nextCart = await this.client.cart.getOrCreate();
    const creation: EmbeddedCheckoutCreation = {
      ...start,
      cart: nextCart.data,
    };
    return { data: creation };
  }

  private checkoutIdempotencyKey(cartId: string): string {
    let key = this.idempotencyKeys.get(cartId);
    if (!key) {
      key = this.client.randomUUID();
      this.idempotencyKeys.set(cartId, key);
    }
    return key;
  }

  private async createEmbeddedCheckoutForCart(
    cart: Cart,
    options: EmbeddedCheckoutOptions,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    const recoveryUrl = normalizeRecoveryUrl(options.recoveryUrl);
    const response = await this.client.request<unknown>(
      `/carts/${encodeURIComponent(cart.id)}/checkout`,
      {
        method: "POST",
        body: toEmbeddedCheckoutRequest(options),
        requiresShopperToken: true,
        idempotencyKey: this.checkoutIdempotencyKey(cart.id),
      },
    );
    const checkout = requireEmbeddedCheckoutSession(
      response,
      recoveryUrl,
    );
    this.activeCheckout = checkout.data;
    activateCheckoutUrl(checkout.data.checkout_url);
    return checkout;
  }

  private async readCheckout(link: CheckoutLink): Promise<CheckoutAccessResponse> {
    const response = await this.client.request<unknown>(
      `/orders/${encodeURIComponent(link.orderId)}/checkout`,
      { checkoutToken: link.checkoutToken },
    );
    const access = requireCheckoutAccess(response);
    if (access.order.id !== link.orderId) throw invalidCheckoutResponse();
    return access;
  }

  private async waitForCheckoutOrder(
    checkout: EmbeddedCheckoutSession,
    options: WaitForCheckoutOrderOptions = {},
  ): Promise<DataEnvelope<CheckoutOrder>> {
    const intervalMs = finiteDuration(
      options.intervalMs,
      DEFAULT_POLL_INTERVAL_MS,
      "intervalMs",
      true,
    );
    const timeoutMs = finiteDuration(
      options.timeoutMs,
      DEFAULT_WAIT_TIMEOUT_MS,
      "timeoutMs",
      false,
    );
    const timeoutController = new AbortController();
    const timeout = setTimeout(
      () => timeoutController.abort(timeoutError(checkout.order_id)),
      timeoutMs,
    );
    const signal = options.signal
      ? AbortSignal.any([options.signal, timeoutController.signal])
      : timeoutController.signal;
    const link: CheckoutLink = {
      orderId: checkout.order_id,
      checkoutToken: checkout.checkout_token,
      url: checkout.checkout_url,
    };

    try {
      for (;;) {
        signal.throwIfAborted();
        const access = await this.readCheckout(link);
        if (isTerminalCheckoutOrder(access.order)) {
          this.activeCheckout = undefined;
          return { data: await this.confirmTerminalOrder(access.order) };
        }
        await delay(intervalMs, signal);
      }
    } finally {
      clearTimeout(timeout);
    }
  }

  private async confirmTerminalOrder(order: CheckoutOrder): Promise<CheckoutOrder> {
    await this.client.recordCheckoutPurchase(order);
    return order;
  }
}

function requireEmbeddedCheckoutSession(
  value: unknown,
  recoveryUrl: string,
): DataEnvelope<EmbeddedCheckoutSession> {
  const data = requireData(value, "invalid_checkout_response");
  if (!isEmbeddedCheckoutApiSession(data)) throw invalidCheckoutResponse();
  return {
    data: {
      ...data,
      checkout_url: buildCheckoutUrl(
        recoveryUrl,
        data.order_id,
        data.checkout_token,
      ),
    },
  };
}

function requireCheckoutAccess(value: unknown): CheckoutAccessResponse {
  const data = requireData(value, "invalid_checkout_response");
  if (!isRecord(data) || !isCheckoutOrder(data.order)) {
    throw invalidCheckoutResponse();
  }
  if (
    data.client_action !== undefined &&
    !isPaymentClientAction(data.client_action)
  ) {
    throw invalidCheckoutResponse();
  }
  return {
    order: data.order,
    ...(data.client_action ? { client_action: data.client_action } : {}),
  };
}

function checkoutSession(
  link: CheckoutLink,
  order: CheckoutOrder,
  action: PaymentClientAction,
): EmbeddedCheckoutSession {
  return {
    order_id: order.id,
    order_number: order.order_number,
    checkout_token: link.checkoutToken,
    checkout_url: link.url,
    client_action: action,
  };
}

function invalidCheckoutResponse(): ChaosApiError {
  return new ChaosApiError(
    502,
    "invalid_checkout_response",
    "storefront checkout response is invalid",
  );
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function isPaymentClientAction(value: unknown): value is PaymentClientAction {
  return (
    isRecord(value) &&
    value.type === "stripe_checkout_embedded" &&
    isNonEmptyString(value.public_key) &&
    isNonEmptyString(value.client_token)
  );
}

function isEmbeddedCheckoutApiSession(
  value: unknown,
): value is EmbeddedCheckoutApiSession {
  return (
    isRecord(value) &&
    isNonEmptyString(value.order_id) &&
    isNonEmptyString(value.order_number) &&
    isNonEmptyString(value.checkout_token) &&
    isPaymentClientAction(value.client_action)
  );
}

function isCheckoutOrder(value: unknown): value is CheckoutOrder {
  return (
    isRecord(value) &&
    isNonEmptyString(value.id) &&
    isNonEmptyString(value.order_number) &&
    isNonEmptyString(value.currency) &&
    ["pending", "confirmed", "cancelled"].includes(String(value.status)) &&
    [
      "pending",
      "paid",
      "failed",
      "expired",
      "partially_refunded",
      "refunded",
    ].includes(String(value.payment_status)) &&
    ["pending", "shipped", "delivered", "cancelled"].includes(
      String(value.fulfillment_status),
    ) &&
    isFiniteNumber(value.subtotal_amount_minor) &&
    isFiniteNumber(value.discount_amount_minor) &&
    isFiniteNumber(value.tax_amount_minor) &&
    isFiniteNumber(value.shipping_amount_minor) &&
    isFiniteNumber(value.total_amount_minor) &&
    isFiniteNumber(value.refunded_amount_minor) &&
    Array.isArray(value.lines) &&
    value.lines.every(isOrderLine) &&
    isNonEmptyString(value.created_at) &&
    isNonEmptyString(value.updated_at)
  );
}

function isOrderLine(value: unknown): boolean {
  return (
    isRecord(value) &&
    isNonEmptyString(value.product_id) &&
    isNonEmptyString(value.product_variant_id) &&
    isNonEmptyString(value.product_title) &&
    isNonEmptyString(value.variant_title) &&
    isFiniteNumber(value.quantity) &&
    isFiniteNumber(value.unit_price_amount_minor) &&
    isFiniteNumber(value.subtotal_amount_minor)
  );
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isTerminalCheckoutOrder(order: CheckoutOrder): boolean {
  if (order.status === "cancelled") return true;
  if (["failed", "expired"].includes(order.payment_status)) return true;
  return (
    order.status === "confirmed" &&
    ["paid", "partially_refunded", "refunded"].includes(order.payment_status)
  );
}

function toEmbeddedCheckoutRequest(
  options: EmbeddedCheckoutOptions,
): EmbeddedCheckoutRequest {
  const body: EmbeddedCheckoutRequest = { payment_provider: "stripe" };
  const attribution = options.attribution ?? defaultAdAttribution();
  if (hasAdAttribution(attribution)) body.attribution = attribution;
  return body;
}

function buildCheckoutUrl(
  base: string,
  orderId: string,
  checkoutToken: string,
): string {
  const url = new URL(base, window.location.origin);
  const fragment = parseFragment(url.hash);
  fragment.params.set(CHECKOUT_ORDER_ID_PARAM, orderId);
  fragment.params.set(CHECKOUT_TOKEN_PARAM, checkoutToken);
  url.hash = fragment.prefix
    ? `${fragment.prefix}?${fragment.params.toString()}`
    : fragment.params.toString();
  return url.toString();
}

function normalizeRecoveryUrl(value: string | undefined): string {
  const url = new URL(value ?? window.location.href, window.location.origin);
  if (url.origin !== window.location.origin) {
    throw new TypeError("recoveryUrl must use the current browser origin");
  }
  return url.toString();
}

function parseCheckoutLink(value: string): CheckoutLink | undefined {
  const url = new URL(value, window.location.origin);
  const { params } = parseFragment(url.hash);
  const orderId = params.get(CHECKOUT_ORDER_ID_PARAM)?.trim();
  const checkoutToken = params.get(CHECKOUT_TOKEN_PARAM)?.trim();
  if (!orderId || !checkoutToken) return undefined;
  return { orderId, checkoutToken, url: url.toString() };
}

function parseFragment(hash: string): {
  prefix: string;
  params: URLSearchParams;
} {
  const fragment = hash.replace(/^#/, "");
  const queryAt = fragment.indexOf("?");
  if (queryAt >= 0) {
    return {
      prefix: fragment.slice(0, queryAt),
      params: new URLSearchParams(fragment.slice(queryAt + 1)),
    };
  }
  if (fragment && !fragment.includes("=")) {
    return { prefix: fragment, params: new URLSearchParams() };
  }
  return { prefix: "", params: new URLSearchParams(fragment) };
}

function activateCheckoutUrl(url: string): void {
  if (window.location.href === url) return;
  window.history?.replaceState?.(window.history.state, "", url);
}

function finiteDuration(
  value: number | undefined,
  fallback: number,
  name: string,
  allowZero: boolean,
): number {
  const duration = value ?? fallback;
  if (
    !Number.isFinite(duration) ||
    duration < 0 ||
    (!allowZero && duration === 0)
  ) {
    throw new TypeError(
      `${name} must be a finite ${allowZero ? "non-negative" : "positive"} number`,
    );
  }
  return duration;
}

function timeoutError(orderId: string): DOMException {
  return new DOMException(
    `Timed out waiting for checkout Order ${orderId}`,
    "TimeoutError",
  );
}

function delay(milliseconds: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) {
    return Promise.reject(
      signal.reason ?? new DOMException("Polling was aborted", "AbortError"),
    );
  }
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    }, milliseconds);
    const abort = () => {
      clearTimeout(timeout);
      reject(
        signal.reason ?? new DOMException("Polling was aborted", "AbortError"),
      );
    };
    signal.addEventListener("abort", abort, { once: true });
  });
}
