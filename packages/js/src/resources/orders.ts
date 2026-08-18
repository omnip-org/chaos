import type { ChaosStorefrontClient } from "../client.js";
import type { DataEnvelope, Order } from "../types.js";

export class OrdersResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  get(orderId: string): Promise<DataEnvelope<Order>> {
    return this.client.request(`/orders/${encodeURIComponent(orderId)}`, {
      method: "GET",
      requiresShopperToken: true,
    });
  }
}
