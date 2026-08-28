import type { ChaosStorefrontClient } from "../client.js";
import type { DataEnvelope, Order, OrderStatus, TrackedOrder } from "../types.js";

export class OrdersResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  get(orderId: string): Promise<DataEnvelope<Order>> {
    return this.client.request<DataEnvelope<Order>>(`/orders/${encodeURIComponent(orderId)}`, {
      method: "GET",
      requiresShopperToken: true,
    }).then((response) => {
      try {
        const analytics = this.client.analytics;
        if (analytics?.recordConfirmedOrder) {
          analytics.recordConfirmedOrder(response.data);
        } else if (
          analytics &&
          response.data.status === "confirmed" &&
          response.data.payment_status === "paid"
        ) {
          // Keep test doubles and older embedded clients source-compatible while
          // the public analytics class adopts recordConfirmedOrder.
          analytics.purchase({
            orderId: response.data.id,
            valueMinor: response.data.total_amount_minor,
            currency: response.data.currency,
            items: response.data.lines.map((line) => ({
              productId: line.product_id,
              productVariantId: line.product_variant_id,
              quantity: line.quantity,
              priceMinor: line.unit_price_amount_minor,
            })),
          });
        }
      } catch {
        // The order read already succeeded; analytics must remain best-effort.
      }
      return response;
    });
  }

  async getStatus(orderId: string): Promise<OrderStatus> {
    const { data } = await this.get(orderId);
    return {
      status: data.status,
      payment_status: data.payment_status,
    };
  }

  getTrackedOrder(trackingToken: string): Promise<DataEnvelope<TrackedOrder>> {
    return this.client.request("/orders/tracking", {
      method: "POST",
      body: { tracking_token: trackingToken },
    });
  }
}
