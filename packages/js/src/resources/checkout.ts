import type { ChaosStorefrontClient } from "../client.js";
import type { Checkout, CreateCheckoutRequest, DataEnvelope, Order } from "../types.js";

export class CheckoutResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  async create(cartId: string, body: CreateCheckoutRequest, idempotencyKey?: string): Promise<DataEnvelope<Checkout>> {
    const response = await this.client.request<DataEnvelope<Checkout>>(`/carts/${encodeURIComponent(cartId)}/checkout`, {
      method: "POST",
      body,
      requiresShopperToken: true,
      idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
    });
    this.client.analytics?.initiateCheckout({ cartId, checkoutId: response.data.id });
    return response;
  }

  get(checkoutId: string): Promise<DataEnvelope<Checkout>> {
    return this.client.request(`/checkouts/${encodeURIComponent(checkoutId)}`, {
      method: "GET",
      requiresShopperToken: true,
    });
  }

  createOrder(checkoutId: string, idempotencyKey?: string): Promise<DataEnvelope<Order>> {
    return this.client.request(`/checkouts/${encodeURIComponent(checkoutId)}/order`, {
      method: "POST",
      requiresShopperToken: true,
      idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
    });
  }
}
