import type { ChaosStorefrontClient } from "../client.js";
import type { Cart, CreateCartRequest, DataEnvelope, SetCartLineRequest } from "../types.js";

export class CartResource {
  private readonly mutationQueues = new Map<string, Promise<unknown>>();

  constructor(private readonly client: ChaosStorefrontClient) {}

  create(body: CreateCartRequest = {}): Promise<DataEnvelope<Cart>> {
    return this.client.request("/carts", {
      method: "POST",
      body,
      requiresShopperToken: true,
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
    expectedVersion?: number,
  ): Promise<DataEnvelope<Cart>> {
    return this.enqueueMutation(cartId, async () => {
      const version = expectedVersion ?? (await this.get(cartId)).data.version;
      return this.setLineRequest(cartId, productVariantId, body, version);
    });
  }

  /** Adds a quantity to a Cart line. The API records the authoritative AddToCart event. */
  async addLine(
    cartId: string,
    productVariantId: string,
    quantity = 1,
  ): Promise<DataEnvelope<Cart>> {
    if (!Number.isInteger(quantity) || quantity < 1) {
      throw new RangeError("quantity must be a positive integer");
    }
    return this.enqueueMutation(cartId, async () => {
      const current = await this.get(cartId);
      const existing = current.data.lines.find((line) => line.product_variant_id === productVariantId);
      const response = await this.setLineRequest(
        cartId,
        productVariantId,
        { quantity: (existing?.quantity ?? 0) + quantity },
        current.data.version,
      );
      return response;
    });
  }

  removeLine(cartId: string, productVariantId: string): Promise<DataEnvelope<Cart>> {
    return this.enqueueMutation(cartId, async () => {
      const current = await this.get(cartId);
      return this.client.request(
        `/carts/${encodeURIComponent(cartId)}/lines/${encodeURIComponent(productVariantId)}`,
        {
          method: "DELETE",
          requiresShopperToken: true,
          ifMatch: String(current.data.version),
        },
      );
    });
  }

  private setLineRequest(
    cartId: string,
    productVariantId: string,
    body: SetCartLineRequest,
    expectedVersion: number,
  ): Promise<DataEnvelope<Cart>> {
    return this.client.request<DataEnvelope<Cart>>(
      `/carts/${encodeURIComponent(cartId)}/lines/${encodeURIComponent(productVariantId)}`,
      {
        method: "PUT",
        body,
        requiresShopperToken: true,
        ifMatch: String(expectedVersion),
      },
    );
  }

  private enqueueMutation<T>(cartId: string, operation: () => Promise<T>): Promise<T> {
    const previous = this.mutationQueues.get(cartId) ?? Promise.resolve();
    const current = previous.catch(() => undefined).then(operation);
    const settled = current.finally(() => {
      if (this.mutationQueues.get(cartId) === settled) this.mutationQueues.delete(cartId);
    });
    this.mutationQueues.set(cartId, settled);
    return settled;
  }
}
