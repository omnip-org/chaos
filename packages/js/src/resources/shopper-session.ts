import type { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import { isRecord, requireData } from "../internal/response.js";
import type { DataEnvelope, ShopperSession } from "../types.js";

export class ShopperSessionResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  /**
   * Creates a new anonymous possession-bound shopper session and persists
   * its token for subsequent Cart/Payment calls. Most callers never need
   * this directly — shopper-scoped requests acquire the session automatically.
   */
  async create(): Promise<DataEnvelope<ShopperSession>> {
    return this.client.issueShopperSession();
  }
}

/** @internal Validates the identity-bearing response before it enters storage. */
export function requireShopperSession(
  value: unknown,
): DataEnvelope<ShopperSession> {
  const data = requireData<unknown>(value, "invalid_shopper_session_response");
  if (
    !isRecord(data) ||
    !isUuid(data.shopper_id) ||
    !isNonEmptyString(data.shopper_token)
  ) {
    throw new ChaosApiError(
      502,
      "invalid_shopper_session_response",
      "storefront shopper-session response is invalid",
    );
  }
  return {
    data: {
      shopper_id: data.shopper_id,
      shopper_token: data.shopper_token,
    },
  };
}

function isUuid(value: unknown): value is string {
  return (
    typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      value,
    )
  );
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}
