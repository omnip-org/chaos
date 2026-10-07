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
  EmbeddedCheckoutStart,
  EmbeddedCheckoutSession,
  PaymentProvider,
} from "../types.js";

interface EmbeddedCheckoutRequest {
  payment_provider: PaymentProvider;
  return_url: string;
  attribution?: CheckoutAttribution;
}

export class PaymentsResource {
  /**
   * One idempotency key per cart id, minted on the first checkout attempt and
   * reused for later attempts in this client. After a page reload the server
   * recovers the same pending checkout from the Cart and request fingerprint,
   * even though the new client mints a different key.
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

  /**
   * Starts checkout from a Cart body the caller just loaded. This removes the
   * redundant pre-checkout GET and returns as soon as the provider handoff is
   * ready. Callers can create the shopper's next Cart in parallel with
   * mounting the payment UI via `cart.getOrCreate()`.
   *
   * The server remains authoritative: it revalidates and atomically locks the
   * Cart, reserves inventory, and applies the idempotency key before returning.
   */
  async createEmbeddedCheckoutFromCart(
    cart: Cart,
    options: EmbeddedCheckoutOptions,
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
    options: EmbeddedCheckoutOptions,
  ): Promise<DataEnvelope<EmbeddedCheckoutCreation>> {
    const { checkout, sourceCart } = await this.client.cart.runExclusive(
      cartId,
      async () => {
        const cart = await this.client.cart.snapshot(cartId);
        const result = await this.createEmbeddedCheckoutForCart(cart, options);
        return { checkout: result, sourceCart: cart };
      },
    );
    const nextCart = await this.client.cart.getOrCreate();
    const creation: EmbeddedCheckoutCreation = {
      checkout: checkout.data,
      source_cart: sourceCart,
      cart: nextCart.data,
    };
    this.client.recordCheckoutCreation(creation);
    return { data: creation };
  }

  private async createEmbeddedCheckoutForCart(
    cart: Cart,
    options: EmbeddedCheckoutOptions,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    const body = toEmbeddedCheckoutRequest(
      options,
      this.client.attributionStorage,
    );
    const response = await this.client.request<unknown>(
      `/carts/${encodeURIComponent(cart.id)}/checkout`,
      {
        method: "POST",
        body,
        requiresShopperToken: true,
        idempotencyKey: this.checkoutIdempotencyKey(cart.id),
      },
    );
    const checkout = requireEmbeddedCheckoutSession(response);
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

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function isEmbeddedCheckoutSession(
  value: unknown,
): value is EmbeddedCheckoutSession {
  if (!isRecord(value) || !isRecord(value.client_action)) return false;
  return (
    isNonEmptyString(value.order_id) &&
    isNonEmptyString(value.order_number) &&
    value.client_action.type === "stripe_checkout_embedded" &&
    isNonEmptyString(value.client_action.public_key) &&
    isNonEmptyString(value.client_action.client_token)
  );
}

function toEmbeddedCheckoutRequest(
  options: EmbeddedCheckoutOptions,
  storage: Pick<Storage, "getItem" | "setItem"> | null,
): EmbeddedCheckoutRequest {
  const body: EmbeddedCheckoutRequest = {
    payment_provider: "stripe",
    return_url: options.returnUrl,
  };
  const attribution = options.attribution ?? defaultAdAttribution(storage);
  if (hasAdAttribution(attribution)) {
    body.attribution = attribution;
  }
  return body;
}
