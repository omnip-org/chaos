import type { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import { isRecord, requireData } from "../internal/response.js";
import { readUtmTags } from "../internal/utm.js";
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
  const attribution = options.attribution ?? defaultAttribution();
  const hasAttribution =
    attribution.source_url ||
    (attribution.utm && Object.keys(attribution.utm).length > 0) ||
    (attribution.meta && (attribution.meta.fbc || attribution.meta.fbp));
  if (hasAttribution) {
    body.attribution = attribution;
  }
  return body;
}

/** Meta's Pixel install sets `_fbp` itself; `_fbc` is chaos-js's own copy of
 * the `fbclid` URL param (see `events/browser.ts`'s `maintainFbcCookie`).
 * Both are plain, non-HttpOnly cookies by Meta's own design, so reading them
 * here needs no extra wiring. `source_url` is the checkout page's own URL —
 * this is Meta CAPI InitiateCheckout's `event_source_url`. */
function defaultAttribution(): CheckoutAttribution {
  const fbc = readCookie("_fbc");
  const fbp = readCookie("_fbp");
  const sourceUrl = typeof window === "undefined" ? undefined : window.location.href;
  const utm = readUtmTags();
  return {
    ...(sourceUrl && { source_url: sourceUrl }),
    ...(utm && { utm }),
    ...((fbc || fbp) && { meta: { ...(fbc && { fbc }), ...(fbp && { fbp }) } }),
  };
}

function readCookie(name: string): string | undefined {
  const cookie =
    typeof document === "undefined" ? undefined : document.cookie;
  if (typeof cookie !== "string") return undefined;
  const prefix = `${name}=`;
  const entry = cookie.split("; ").find((value) => value.startsWith(prefix));
  return entry ? decodeURIComponent(entry.slice(prefix.length)) : undefined;
}
