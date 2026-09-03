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
}
