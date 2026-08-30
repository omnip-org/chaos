import type { ChaosStorefrontClient } from "../client.js";
import type { DataEnvelope, TrackedOrder } from "../types.js";

export class OrdersResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  getTrackedOrder(trackingToken: string): Promise<DataEnvelope<TrackedOrder>> {
    return this.client.request("/orders/tracking", {
      method: "POST",
      body: { tracking_token: trackingToken },
    });
  }
}
