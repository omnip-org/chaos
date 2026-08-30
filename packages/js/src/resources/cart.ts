import { ChaosApiError } from "../errors.js";
import type { ChaosStorefrontClient } from "../client.js";
import type {
  Cart,
  CreateCartRequest,
  DataEnvelope,
  SetCartLineRequest,
} from "../types.js";

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

  /**
   * Reads a cart only when it is still active. A missing, locked, or
   * abandoned cart returns null without creating a replacement.
   *
   * Invalid shopper credentials are cleared from the configured token
   * storage, but this method never mints a new identity as a side effect.
   */
  async getActive(cartId: string): Promise<DataEnvelope<Cart> | null> {
    if (!this.client.getShopperToken()) return null;
    try {
      const response = await this.get(cartId);
      return response.data.status === "active" ? response : null;
    } catch (error) {
      if (
        error instanceof ChaosApiError &&
        (error.status === 401 || error.status === 403 || error.status === 404)
      ) {
        if (error.status === 401 || error.status === 403) {
          this.client.setShopperToken(null);
        }
        return null;
      }
      throw error;
    }
  }

  /**
   * Returns an active cart for the current shopper, creating one when the
   * supplied cart id is stale or belongs to a locked checkout. Shopper
   * identity recovery is explicit and persists through the client's configured
   * token storage.
   */
  async getOrCreate(cartId?: string): Promise<DataEnvelope<Cart>> {
    if (cartId) {
      const current = await this.getActive(cartId);
      if (current) return current;
    }

    if (!this.client.getShopperToken()) {
      await this.client.acquireShopperToken();
    }

    try {
      return await this.create();
    } catch (error) {
      if (
        !(error instanceof ChaosApiError) ||
        (error.status !== 401 && error.status !== 403)
      ) {
        throw error;
      }
      this.client.setShopperToken(null);
      await this.client.acquireShopperToken();
      return this.create();
    }
  }

  async setLine(
    cartId: string,
    productVariantId: string,
    body: SetCartLineRequest,
    expectedVersion?: number,
  ): Promise<DataEnvelope<Cart>> {
    return this.enqueueMutation(cartId, async () => {
      const current =
        this.client.analytics || expectedVersion === undefined
          ? await this.get(cartId)
          : undefined;
      const existingQuantity =
        current?.data.lines.find(
          (line) => line.product_variant_id === productVariantId,
        )?.quantity ?? 0;
      const response = await this.setLineRequest(
        cartId,
        productVariantId,
        body,
        expectedVersion ?? current!.data.version,
      );
      if (body.quantity > existingQuantity) {
        projectAddToCart(
          this.client,
          response.data,
          productVariantId,
          body.quantity - existingQuantity,
        );
      }
      return response;
    });
  }

  /** Adds a quantity to a Cart line and records AddToCart after success. */
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
      const existing = current.data.lines.find(
        (line) => line.product_variant_id === productVariantId,
      );
      const response = await this.setLineRequest(
        cartId,
        productVariantId,
        {
          quantity: (existing?.quantity ?? 0) + quantity,
        },
        current.data.version,
      );
      if (this.client.analytics) {
        projectAddToCart(
          this.client,
          response.data,
          productVariantId,
          quantity,
        );
      }
      return response;
    });
  }

  removeLine(
    cartId: string,
    productVariantId: string,
  ): Promise<DataEnvelope<Cart>> {
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

  private enqueueMutation<T>(
    cartId: string,
    operation: () => Promise<T>,
  ): Promise<T> {
    const previous = this.mutationQueues.get(cartId) ?? Promise.resolve();
    const current = previous.catch(() => undefined).then(operation);
    const settled = current.finally(() => {
      if (this.mutationQueues.get(cartId) === settled)
        this.mutationQueues.delete(cartId);
    });
    this.mutationQueues.set(cartId, settled);
    return settled;
  }
}

function projectAddToCart(
  client: ChaosStorefrontClient,
  cart: Cart,
  productVariantId: string,
  quantity: number,
): void {
  try {
    const line = cart.lines.find(
      (candidate) => candidate.product_variant_id === productVariantId,
    );
    if (!line) return;
    client.analytics?.recordAddToCart({
      cartId: cart.id,
      productId: line.product_id,
      productVariantId: line.product_variant_id,
      quantity,
      priceMinor: line.unit_price_amount_minor,
      valueMinor: line.unit_price_amount_minor * quantity,
      currency: cart.currency,
    });
  } catch {
    // Analytics recording and provider projection are best-effort after the
    // successful cart mutation.
  }
}
