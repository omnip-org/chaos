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
  create(): Promise<DataEnvelope<ShopperSession>> {
    return this.client.request("/shopper-sessions", { method: "POST" });
  }
}
