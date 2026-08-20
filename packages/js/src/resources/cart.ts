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

  async setLine(
    cartId: string,
    productVariantId: string,
    body: SetCartLineRequest,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<Cart>> {
    const response = await this.client.request<DataEnvelope<Cart>>(
      `/carts/${encodeURIComponent(cartId)}/lines/${encodeURIComponent(productVariantId)}`,
      {
        method: "PUT",
        body,
        requiresShopperToken: true,
        idempotencyKey: idempotencyKey ?? this.client.randomUUID(),
      },
    );
    return response;
  }

  /** Adds a quantity to a Cart line and records one accurate AddToCart event after success. */
  async addLine(
    cartId: string,
    productVariantId: string,
    quantity = 1,
    idempotencyKey?: string,
  ): Promise<DataEnvelope<Cart>> {
    if (!Number.isInteger(quantity) || quantity < 1) {
      throw new RangeError("quantity must be a positive integer");
    }
    const current = await this.get(cartId);
    const existing = current.data.lines.find((line) => line.product_variant_id === productVariantId);
    const response = await this.setLine(
      cartId,
      productVariantId,
      { quantity: (existing?.quantity ?? 0) + quantity },
      idempotencyKey,
    );
    this.client.analytics?.addToCart({ cartId, productVariantId, quantity });
    return response;
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
