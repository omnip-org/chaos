import type { ChaosStorefrontClient } from "../client.js";
import type { DataEnvelope, ShopperSession } from "../types.js";

export class ShopperSessionResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  /**
   * Creates a new anonymous possession-bound shopper session and persists
   * its token for subsequent Cart/Checkout calls. Most callers never need
   * this directly — cart.create()/setLine() etc. acquire one automatically
   * the first time it's needed.
   */
  async create(): Promise<DataEnvelope<ShopperSession>> {
    const response = await this.client.request<DataEnvelope<ShopperSession>>("/shopper-sessions", {
      method: "POST",
    });
    this.client.setShopperToken(response.data.shopper_token);
    return response;
  }
}
