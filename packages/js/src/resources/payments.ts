import type { ChaosStorefrontClient } from "../client.js";
import type {
  CreateEmbeddedCheckoutRequest,
  DataEnvelope,
  EmbeddedCheckoutSession,
  PreparedAnalyticsEvent,
} from "../types.js";

export class PaymentsResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  async createEmbeddedCheckout(
    cartId: string,
    body: CreateEmbeddedCheckoutRequest,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    const resolvedIdempotencyKey = idempotencyKey ?? this.client.randomUUID();
    let event: PreparedAnalyticsEvent | undefined;
    if (
      typeof this.client.analytics?.prepareCommerceEvent === "function"
    ) {
      let properties: Record<string, unknown> = { cart_id: cartId };
      try {
        const cart = await this.client.cart.get(cartId);
        properties = {
          cart_id: cart.data.id,
          value_minor: cart.data.subtotal_amount_minor,
          currency: cart.data.currency,
          items: cart.data.lines.map((line) => ({
            item_id: line.product_variant_id,
            quantity: line.quantity,
            price_minor: line.unit_price_amount_minor,
          })),
        };
      } catch {
        // The checkout response remains the authority if this optional read
        // is unavailable; the event still carries attribution.
      }
      try {
        event = this.client.analytics.prepareCommerceEvent(
          "initiate_checkout",
          properties,
          isUuid(resolvedIdempotencyKey) ? resolvedIdempotencyKey : undefined,
        );
      } catch {
        // Analytics preparation must not prevent a valid checkout request.
        event = undefined;
      }
    }
    return this.client
      .request<DataEnvelope<EmbeddedCheckoutSession>>(
        `/carts/${encodeURIComponent(cartId)}/checkout`,
        {
          method: "POST",
          body,
          requiresShopperToken: true,
          idempotencyKey: resolvedIdempotencyKey,
        },
      )
      .then((response) => {
        if (
          event?.event_name === "initiate_checkout" &&
          this.client.analytics
        ) {
          try {
            this.client.analytics.sendCommerceEvent(event, {
              cart_id: cartId,
              order_id: response.data.order_id,
            });
          } catch {
            // The checkout session already exists; provider projection is best-effort.
          }
        }
        return response;
      });
  }
}

function isUuid(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
    value,
  );
}
