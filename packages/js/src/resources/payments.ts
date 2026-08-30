import type { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import type {
  Cart,
  DataEnvelope,
  EmbeddedCheckoutOptions,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutSession,
  PendingPaymentOrder,
} from "../types.js";

interface EmbeddedCheckoutRequest {
  email?: string;
  payment_provider: "stripe";
  return_url: string;
}

export class PaymentsResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  async createEmbeddedCheckout(
    cartId: string,
    options: EmbeddedCheckoutOptions,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    const cart = await this.client.cart.get(cartId);
    return this.createEmbeddedCheckoutForCart(cart.data, options);
  }

  async createEmbeddedCheckoutWithCart(
    cartId: string,
    options: EmbeddedCheckoutOptions,
  ): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
    const cart = await this.client.cart.get(cartId);
    const checkout = await this.createEmbeddedCheckoutForCart(cart.data, options);
    const nextCart = await this.client.cart.getOrCreate();
    return {
      data: {
        checkout: checkout.data,
        cart: nextCart.data,
      },
    };
  }

  async resumeEmbeddedCheckout(
    orderId: string,
    options: Pick<EmbeddedCheckoutOptions, "returnUrl"> | undefined = undefined,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    if (!orderId.trim()) {
      throw new TypeError("orderId is required");
    }
    const response = await this.client.request<unknown>(
      `/orders/${encodeURIComponent(orderId)}/checkout`,
      {
        method: "POST",
        body: options?.returnUrl ? { return_url: options.returnUrl } : {},
        requiresShopperToken: true,
      },
    );
    return requireEmbeddedCheckoutSession(response);
  }

  async listPendingPaymentOrders(): Promise<DataEnvelope<PendingPaymentOrder[]>> {
    const response = await this.client.request<unknown>(
      "/orders/pending-payment",
      { method: "GET", requiresShopperToken: true },
    );
    return requirePendingPaymentOrders(response);
  }

  private async createEmbeddedCheckoutForCart(
    cart: Cart,
    options: EmbeddedCheckoutOptions,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    const body = toEmbeddedCheckoutRequest(options);
    const response = await this.client.request<unknown>(
      `/carts/${encodeURIComponent(cart.id)}/checkout`,
      {
        method: "POST",
        body,
        requiresShopperToken: true,
        idempotencyKey: checkoutIdempotencyKey(cart, body),
      },
    );
    const checkout = requireEmbeddedCheckoutSession(response);

    if (cart.status === "active") {
      try {
        this.client.analytics?.recordInitiateCheckout({
          cartId: cart.id,
          orderId: checkout.data.order_id,
          valueMinor: cart.subtotal_amount_minor,
          currency: cart.currency,
          items: cart.lines.map((line) => ({
            productId: line.product_id,
            productVariantId: line.product_variant_id,
            quantity: line.quantity,
            priceMinor: line.unit_price_amount_minor,
          })),
        });
      } catch {
        // The checkout session already exists; analytics is best-effort.
      }
    }
    return checkout;
  }
}

function requireEmbeddedCheckoutSession(
  value: unknown,
): DataEnvelope<EmbeddedCheckoutSession> {
  const data = requireData(value, "invalid_checkout_response");
  if (!isEmbeddedCheckoutSession(data)) {
    throw new ChaosApiError(
      502,
      "invalid_checkout_response",
      "storefront checkout response is invalid",
    );
  }
  return { data };
}

function requirePendingPaymentOrders(
  value: unknown,
): DataEnvelope<PendingPaymentOrder[]> {
  const data = requireData(value, "invalid_pending_payment_orders_response");
  if (
    !Array.isArray(data) ||
    !data.every(isPendingPaymentOrder)
  ) {
    throw new ChaosApiError(
      502,
      "invalid_pending_payment_orders_response",
      "storefront pending payment orders response is invalid",
    );
  }
  return { data };
}

function requireData(value: unknown, code: string): unknown {
  if (!isRecord(value) || !("data" in value) || value.data === null) {
    throw new ChaosApiError(502, code, "storefront response is invalid");
  }
  return value.data;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function isEmbeddedCheckoutSession(
  value: unknown,
): value is EmbeddedCheckoutSession {
  if (!isRecord(value) || !isRecord(value.client_action)) return false;
  return (
    isNonEmptyString(value.order_id) &&
    isNonEmptyString(value.source_cart_id) &&
    value.client_action.type === "mount_embedded_checkout" &&
    isNonEmptyString(value.client_action.public_key) &&
    isNonEmptyString(value.client_action.client_token)
  );
}

function isPendingPaymentOrder(value: unknown): value is PendingPaymentOrder {
  return (
    isRecord(value) &&
    isNonEmptyString(value.order_id) &&
    isNonEmptyString(value.source_cart_id) &&
    isNonEmptyString(value.currency) &&
    Number.isSafeInteger(value.subtotal_amount_minor) &&
    isNonEmptyString(value.created_at) &&
    isNonEmptyString(value.updated_at)
  );
}

function toEmbeddedCheckoutRequest(
  options: EmbeddedCheckoutOptions,
): EmbeddedCheckoutRequest {
  const body: EmbeddedCheckoutRequest = {
    payment_provider: "stripe",
    return_url: options.returnUrl,
  };
  if (options.email) body.email = options.email;
  return body;
}

function checkoutIdempotencyKey(
  cart: Cart,
  request: EmbeddedCheckoutRequest,
): string {
  // The source Cart changes status and version when checkout starts. Exclude
  // those server-side lifecycle fields so a lost response can safely retry
  // with the same key; include the actual cart snapshot so a real cart edit
  // receives a new key.
  return stableUuid(
    JSON.stringify([
      "embedded-checkout-v2",
      cart.id,
      cart.price_list_id,
      cart.currency,
      cart.lines.map((line) => [
        line.product_id,
        line.product_variant_id,
        line.product_title,
        line.variant_title,
        line.sku,
        line.track_inventory,
        line.quantity,
        line.unit_price_amount_minor,
      ]),
      request,
    ]),
  );
}

function stableUuid(input: string): string {
  const hashes = [
    2_166_136_261, 2_246_822_519, 3_266_489_909, 3_432_918_353,
  ].map((seed) => {
    let hash = seed >>> 0;
    for (let index = 0; index < input.length; index += 1) {
      hash ^= input.charCodeAt(index);
      hash = Math.imul(hash, 16_777_619);
      hash ^= hash >>> 13;
    }
    return hash >>> 0;
  });
  const bytes = new Uint8Array(16);
  const view = new DataView(bytes.buffer);
  hashes.forEach((hash, index) => view.setUint32(index * 4, hash));
  bytes[6] = (bytes[6]! & 0x0f) | 0x50;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex
    .slice(6, 8)
    .join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10).join("")}`;
}
