import type { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import {
  defaultAdAttribution,
  hasAdAttribution,
} from "../internal/attribution.js";
import { isRecord, requireData } from "../internal/response.js";
import type {
  Cart,
  CheckoutAttribution,
  DataEnvelope,
  EmbeddedCheckoutOptions,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutSession,
} from "../types.js";

interface EmbeddedCheckoutRequest {
  payment_provider: "stripe";
  return_url: string;
  attribution?: CheckoutAttribution;
}

export class PaymentsResource {
  /**
   * One idempotency key per cart id, minted on the first checkout attempt and
   * reused for every later attempt on that cart. A dropped response therefore
   * retries under the same key (the server returns the existing checkout
   * instead of starting a second one); a cart id is single-use for checkout
   * anyway, since the first success locks it.
   */
  private readonly idempotencyKeys = new Map<string, string>();

  constructor(private readonly client: ChaosStorefrontClient) {}

  private checkoutIdempotencyKey(cartId: string): string {
    let key = this.idempotencyKeys.get(cartId);
    if (!key) {
      key = this.client.randomUUID();
      this.idempotencyKeys.set(cartId, key);
    }
    return key;
  }

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
    const creation: EmbeddedCheckoutCreation = {
      checkout: checkout.data,
      source_cart: sourceCart,
      cart: nextCart.data,
      event_id: checkout.data.event_id,
    };
    this.client.recordCheckoutCreation(creation);
    return { data: creation };
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
        idempotencyKey: this.checkoutIdempotencyKey(cart.id),
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
    isNonEmptyString(value.event_id) &&
    value.client_action.type === "stripe_checkout_embedded" &&
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
  const attribution = options.attribution ?? defaultAdAttribution();
  if (hasAdAttribution(attribution)) {
    body.attribution = attribution;
  }
  return body;
}
