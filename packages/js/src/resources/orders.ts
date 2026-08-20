import type { ChaosStorefrontClient } from "../client.js";
import type { DataEnvelope, Order, OrderTrackingSession } from "../types.js";

export class OrdersResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  get(orderId: string): Promise<DataEnvelope<Order>> {
    return this.client.request<DataEnvelope<Order>>(`/orders/${encodeURIComponent(orderId)}`, {
      method: "GET",
      requiresShopperToken: true,
    }).then((response) => {
      if (response.data.status === "confirmed") {
        this.client.analytics?.purchase({
          orderId: response.data.id,
          valueMinor: response.data.total_amount_minor,
          currency: response.data.currency,
          items: response.data.lines.map((line) => ({
            itemId: line.product_variant_id,
            quantity: line.quantity,
            priceMinor: line.unit_price_amount_minor,
          })),
        });
      }
      return response;
    });
  }

  exchangeTrackingKey(trackingKey: string): Promise<DataEnvelope<OrderTrackingSession>> {
    return this.client.request("/order-tracking-sessions", {
      method: "POST",
      body: { tracking_key: trackingKey },
    });
  }

  getTrackedOrder(accessToken: string): Promise<DataEnvelope<Order>> {
    return this.client.request("/order-tracking-orders", {
      method: "POST",
      body: { access_token: accessToken },
    });
  }
}
