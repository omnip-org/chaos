import type { ChaosStorefrontClient } from "../client.js";
import type {
  Cart,
  DataEnvelope,
  CheckoutAttempt,
  EmbeddedCheckoutOptions,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutSession,
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
    const successorCart = await this.client.cart.get(
      checkout.data.successor_cart_id,
    );
    return {
      data: {
        checkout: checkout.data,
        cart: successorCart.data,
      },
    };
  }

  async resumeEmbeddedCheckout(
    checkoutAttemptId: string,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    if (!checkoutAttemptId.trim()) {
      throw new TypeError("checkoutAttemptId is required");
    }
    return this.client.request<DataEnvelope<EmbeddedCheckoutSession>>(
      `/checkout-attempts/${encodeURIComponent(checkoutAttemptId)}/resume`,
      {
        method: "POST",
        requiresShopperToken: true,
      },
    );
  }

  listCheckoutAttempts(): Promise<DataEnvelope<CheckoutAttempt[]>> {
    return this.client.request<DataEnvelope<CheckoutAttempt[]>>(
      "/checkout-attempts",
      { method: "GET", requiresShopperToken: true },
    );
  }

  getCheckoutAttempt(
    checkoutAttemptId: string,
  ): Promise<DataEnvelope<CheckoutAttempt>> {
    if (!checkoutAttemptId.trim()) {
      throw new TypeError("checkoutAttemptId is required");
    }
    return this.client.request<DataEnvelope<CheckoutAttempt>>(
      `/checkout-attempts/${encodeURIComponent(checkoutAttemptId)}`,
      { method: "GET", requiresShopperToken: true },
    );
  }

  private async createEmbeddedCheckoutForCart(
    cart: Cart,
    options: EmbeddedCheckoutOptions,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    const body = toEmbeddedCheckoutRequest(options);
    const response = await this.client.request<
      DataEnvelope<EmbeddedCheckoutSession>
    >(`/carts/${encodeURIComponent(cart.id)}/checkout`, {
      method: "POST",
      body,
      requiresShopperToken: true,
      idempotencyKey: checkoutIdempotencyKey(cart, body),
    });

    try {
      this.client.analytics?.recordInitiateCheckout({
        cartId: cart.id,
        orderId: response.data.order_id,
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
    return response;
  }
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
