import type { ChaosStorefrontClient } from "../client.js";
import type { DataEnvelope, OrderLookup } from "../types.js";

export interface OrderLookupParams {
  orderNumber: string;
  email: string;
}

export class OrdersResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  lookupOrder(params: OrderLookupParams): Promise<DataEnvelope<OrderLookup>> {
    return this.client.request("/orders/lookup", {
      method: "POST",
      body: { order_number: params.orderNumber, email: params.email },
    });
  }

  /**
   * Projects a confirmed, paid order to Meta Pixel/GA4 — never inferred from
   * browser activity. Call this on a return page right after `lookupOrder`.
   */
  recordConfirmedPurchase(
    order: Pick<
      OrderLookup,
      "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
    >,
  ): void {
    this.client.recordConfirmedPurchase(order);
  }
}
