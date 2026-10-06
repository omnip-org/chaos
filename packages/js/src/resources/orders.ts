import type { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import type { ConfirmedPurchaseOrderInput, DataEnvelope, OrderLookup, OwnOrder } from "../types.js";

export interface OrderLookupParams {
  orderNumber: string;
  email: string;
}

export class OrdersResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  lookupOrder(params: OrderLookupParams): Promise<DataEnvelope<OrderLookup>> {
    return this.client.request("/orders/search", {
      query: { order_number: params.orderNumber, email: params.email },
    });
  }

  /** Reads this shopper's existing Order and records a fresh paid checkout in the browser. */
  async getCheckoutOrder(orderId: string): Promise<DataEnvelope<OwnOrder>> {
    if (!this.client.getShopperToken()) {
      throw new ChaosApiError(401, "shopper_token_required", "the checkout shopper token is missing");
    }
    const result = await this.client.request<DataEnvelope<OwnOrder>>(
      `/orders/${encodeURIComponent(orderId)}/details`,
      { requiresShopperToken: true, retryShopperToken: false },
    );
    await this.client.recordCheckoutPurchase(result.data);
    return result;
  }

  /**
   * Manual projection for integrations that already verified a current Order.
   * Checkout return pages should use `getCheckoutOrder` instead.
   */
  recordConfirmedPurchase(order: ConfirmedPurchaseOrderInput): void {
    this.client.recordConfirmedPurchase(order);
  }
}
