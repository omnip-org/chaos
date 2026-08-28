import { ChaosApiError } from "../errors.js";
import type { ChaosStorefrontClient } from "../client.js";
import type {
  Cart,
  CreateCartRequest,
  DataEnvelope,
  PreparedAnalyticsEvent,
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
   * Reads a cart only when it is still active. A missing, completed, or
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
   * supplied cart id is stale or belongs to a completed checkout. Shopper
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
      const event =
        body.quantity > existingQuantity
          ? prepareAddToCartEvent(
              this.client,
              productVariantId,
              body.quantity - existingQuantity,
            )
          : undefined;
      const response = await this.setLineRequest(
        cartId,
        productVariantId,
        body,
        expectedVersion ?? current!.data.version,
      );
      if (
        event?.event_name === "add_to_cart" &&
        body.quantity > existingQuantity
      ) {
        projectAddToCart(
          this.client,
          event,
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
      const event = prepareAddToCartEvent(
        this.client,
        productVariantId,
        quantity,
      );
      const response = await this.setLineRequest(
        cartId,
        productVariantId,
        {
          quantity: (existing?.quantity ?? 0) + quantity,
        },
        current.data.version,
      );
      // The business request stays analytics-agnostic. Browser clients record
      // the prepared event through /analytics/events only after success.
      if (event?.event_name === "add_to_cart" && this.client.analytics) {
        projectAddToCart(
          this.client,
          event,
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
  event: PreparedAnalyticsEvent,
  cart: Cart,
  productVariantId: string,
  quantity: number,
): void {
  try {
    const line = cart.lines.find(
      (candidate) => candidate.product_variant_id === productVariantId,
    );
    const canonicalVariantId = line?.product_variant_id ?? productVariantId;
    const properties: Record<string, unknown> = {
      cart_id: cart.id,
      product_variant_id: canonicalVariantId,
      quantity,
    };
    if (line) {
      properties.product_id = line.product_id;
      properties.value_minor = line.unit_price_amount_minor * quantity;
      properties.currency = cart.currency;
      properties.items = [
        {
          product_id: line.product_id,
          product_variant_id: canonicalVariantId,
          quantity,
          price_minor: line.unit_price_amount_minor,
        },
      ];
    }
    client.analytics?.sendCommerceEvent(event, properties);
  } catch {
    // Analytics recording and provider projection are best-effort after the
    // successful cart mutation; the SDK persists failed delivery for retry.
  }
}

function prepareAddToCartEvent(
  client: ChaosStorefrontClient,
  productVariantId: string,
  quantity: number,
): PreparedAnalyticsEvent | undefined {
  if (typeof client.analytics?.prepareCommerceEvent !== "function") {
    return undefined;
  }
  try {
    return client.analytics.prepareCommerceEvent("add_to_cart", {
      product_variant_id: productVariantId,
      quantity,
    });
  } catch {
    // Analytics preparation must not turn a valid cart mutation into a failure.
    return undefined;
  }
}
