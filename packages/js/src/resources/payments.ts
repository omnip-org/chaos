import type { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import { fnv1a32 } from "../internal/hash.js";
import { isRecord, requireData } from "../internal/response.js";
import type {
  Cart,
  DataEnvelope,
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
    return this.client.cart.runExclusive(cartId, async () => {
      const cart = await this.client.cart.get(cartId);
      return this.createEmbeddedCheckoutForCart(cart.data, options);
    });
  }

  async createEmbeddedCheckoutWithCart(
    cartId: string,
    options: EmbeddedCheckoutOptions,
  ): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
    const { checkout, sourceCart } = await this.client.cart.runExclusive(
      cartId,
      async () => {
        const cart = await this.client.cart.get(cartId);
        const result = await this.createEmbeddedCheckoutForCart(cart.data, options);
        return { checkout: result, sourceCart: cart.data };
      },
    );
    const nextCart = await this.client.cart.getOrCreate();
    return {
      data: {
        checkout: checkout.data,
        source_cart: sourceCart,
        cart: nextCart.data,
      },
    };
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
    return requireEmbeddedCheckoutSession(response);
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

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function isEmbeddedCheckoutSession(
  value: unknown,
): value is EmbeddedCheckoutSession {
  if (!isRecord(value) || !isRecord(value.client_action)) return false;
  return (
    isNonEmptyString(value.order_number) &&
    value.client_action.type === "mount_embedded_checkout" &&
    isNonEmptyString(value.client_action.public_key) &&
    isNonEmptyString(value.client_action.client_token)
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
      "embedded-checkout-v3",
      cart.id,
      cart.currency,
      cart.lines.map((line) => [
        line.product_id,
        line.product_variant_id,
        line.product_title,
        line.variant_title,
        line.sku,
        line.quantity,
        line.unit_price_amount_minor,
      ]),
      request,
    ]),
  );
}

const STABLE_UUID_SEEDS = [
  2_166_136_261, 2_246_822_519, 3_266_489_909, 3_432_918_353,
];

function stableUuid(input: string): string {
  const hashes = STABLE_UUID_SEEDS.map((seed) => fnv1a32(input, seed, true));
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
