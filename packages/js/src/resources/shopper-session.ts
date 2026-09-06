import type { ChaosStorefrontClient } from "../client.js";
import { utmRequestParams, type UtmRequestParams } from "../internal/utm.js";
import type { DataEnvelope, ShopperSession } from "../types.js";

export class ShopperSessionResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  /**
   * Creates a new anonymous possession-bound shopper session and persists
   * its token for subsequent Cart/Payment calls. Most callers never need
   * this directly — shopper-scoped requests acquire the session automatically.
   */
  async create(): Promise<DataEnvelope<ShopperSession>> {
    const response = await this.client.request<DataEnvelope<ShopperSession>, UtmRequestParams>(
      "/shopper/sessions",
      { method: "POST", query: utmRequestParams() },
    );
    this.client.setShopperToken(response.data.shopper_token);
    return response;
  }
}
