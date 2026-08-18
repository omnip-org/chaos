import type { ChaosStorefrontClient } from "../client.js";
import type { Cart, CreateCartRequest, DataEnvelope, SetCartLineRequest, ShippingOption } from "../types.js";

export class CartResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  create(body: CreateCartRequest = {}, idempotencyKey?: string): Promise<DataEnvelope<Cart>> {
    return this.client.request("/carts", {
      method: "POST",
      body,
      requiresShopperToken: true,
      idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
    });
  }

  get(cartId: string): Promise<DataEnvelope<Cart>> {
    return this.client.request(`/carts/${encodeURIComponent(cartId)}`, {
      method: "GET",
      requiresShopperToken: true,
    });
  }

  setLine(
    cartId: string,
    productVariantId: string,
    body: SetCartLineRequest,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<Cart>> {
    return this.client.request(
      `/carts/${encodeURIComponent(cartId)}/lines/${encodeURIComponent(productVariantId)}`,
      {
        method: "PUT",
        body,
        requiresShopperToken: true,
        idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
      },
    );
  }

  removeLine(cartId: string, productVariantId: string, idempotencyKey?: string): Promise<DataEnvelope<Cart>> {
    return this.client.request(
      `/carts/${encodeURIComponent(cartId)}/lines/${encodeURIComponent(productVariantId)}`,
      {
        method: "DELETE",
        requiresShopperToken: true,
        idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
      },
    );
  }

  quoteShippingOptions(cartId: string, destinationCountry: string): Promise<DataEnvelope<ShippingOption[]>> {
    return this.client.request(`/carts/${encodeURIComponent(cartId)}/shipping-options`, {
      method: "POST",
      body: { destination_country: destinationCountry },
      requiresShopperToken: true,
    });
  }
}
