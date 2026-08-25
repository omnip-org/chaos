import type { ChaosStorefrontClient } from "../client.js";
import type {
  CreateEmbeddedCheckoutRequest,
  DataEnvelope,
  EmbeddedCheckoutSession,
} from "../types.js";

export class PaymentsResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  createEmbeddedCheckout(
    cartId: string,
    body: CreateEmbeddedCheckoutRequest,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<EmbeddedCheckoutSession>> {
    return this.client.request<DataEnvelope<EmbeddedCheckoutSession>>(
      `/carts/${encodeURIComponent(cartId)}/embedded-checkout`,
      {
        method: "POST",
        body,
        requiresShopperToken: true,
        idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
      },
    );
  }

}
