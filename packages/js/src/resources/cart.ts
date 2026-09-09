import { ChaosApiError } from "../errors.js";
import type { ChaosStorefrontClient } from "../client.js";
import { adAttributionBody } from "../internal/attribution.js";
import type {
  Cart,
  DataEnvelope,
  SetCartLineRequest,
} from "../types.js";

/**
 * A cart body counts as active — safe to reuse for a later mutation and to
 * persist as the `resume()` target — when the server says so, or when status
 * is absent (older responses and test doubles omit it).
 */
function isActiveCart(cart: Cart): boolean {
  return cart.status === undefined || cart.status === "active";
}

/**
 * The 409 the Storefront API returns from a line mutation when the cart has
 * been locked or completed by checkout. The recovery is always the same:
 * move to the shopper's current active cart and try once more.
 */
function isCartNotActive(error: unknown): error is ChaosApiError {
  return (
    error instanceof ChaosApiError &&
    error.status === 409 &&
    error.code === "cart_not_active"
  );
}

export class CartResource {
  private readonly mutationQueues = new Map<string, Promise<unknown>>();
  /** Freshest cart body the API returned per id, with the time it arrived. */
  private readonly snapshots = new Map<string, { cart: Cart; at: number }>();
  private pendingWarmup: Promise<DataEnvelope<Cart>> | null = null;

  constructor(
    private readonly client: ChaosStorefrontClient,
    /**
     * How long a remembered cart body satisfies the pre-write read in
     * `addLine`/`setLine`/`removeLine` instead of a separate `GET /carts/{id}`.
     * 0 disables the cache — every mutation re-reads, matching the pre-cache
     * behaviour.
     */
    private readonly snapshotTtlMs = 30_000,
  ) {}

  /**
   * Records a cart body as the freshest known state for its id so the read
   * that a line mutation does before its write can skip `GET /carts/{id}`, and
   * persists an active cart id for `resume()`. Inactive carts are neither
   * cached nor persisted.
   */
  private remember(cart: Cart): Cart {
    if (!isActiveCart(cart)) return cart;
    if (this.snapshotTtlMs > 0) {
      this.snapshots.set(cart.id, { cart, at: this.client.now() });
    }
    this.client.setStoredCartId(cart.id);
    return cart;
  }

  /** Drops a cart from the cache and, if it is the persisted one, from storage. */
  private forget(cartId: string): void {
    this.snapshots.delete(cartId);
    if (this.client.getStoredCartId() === cartId) {
      this.client.setStoredCartId(null);
    }
  }

  /** The remembered cart body while still within the TTL, otherwise a fresh `GET`. */
  private async snapshot(cartId: string): Promise<Cart> {
    const hit = this.snapshots.get(cartId);
    if (hit && this.client.now() - hit.at < this.snapshotTtlMs) {
      return hit.cart;
    }
    return (await this.get(cartId)).data;
  }

  async create(body: Record<string, never> = {}): Promise<DataEnvelope<Cart>> {
    const response = await this.client.request<DataEnvelope<Cart>>("/carts", {
      method: "POST",
      body,
      requiresShopperToken: true,
    });
    this.remember(response.data);
    return response;
  }

  async get(cartId: string): Promise<DataEnvelope<Cart>> {
    const response = await this.client.request<DataEnvelope<Cart>>(
      `/carts/${encodeURIComponent(cartId)}`,
      { method: "GET", requiresShopperToken: true },
    );
    this.remember(response.data);
    return response;
  }

  /**
   * Reads a cart only when it is still active. A missing, locked, or
   * abandoned cart returns null without creating a replacement, and is
   * dropped from the cache and the persisted id.
   *
   * Invalid shopper credentials are cleared from the configured token
   * storage, but this method never mints a new identity as a side effect.
   */
  async getActive(cartId: string): Promise<DataEnvelope<Cart> | null> {
    if (!this.client.getShopperToken()) return null;
    try {
      const response = await this.get(cartId);
      if (response.data.status === "active") return response;
      this.forget(cartId);
      return null;
    } catch (error) {
      if (
        error instanceof ChaosApiError &&
        (error.status === 401 || error.status === 403 || error.status === 404)
      ) {
        if (error.status === 401 || error.status === 403) {
          this.client.setShopperToken(null);
        }
        this.forget(cartId);
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

  /**
   * Resumes the last active cart this client persisted (see
   * `ClientOptions.storage`), creating a fresh one only when there is no
   * stored id or it is no longer active. The stored id is still validated with
   * a `GET`, so a stale id can never surface a locked or foreign cart.
   */
  async resume(): Promise<DataEnvelope<Cart>> {
    const stored = this.client.getStoredCartId();
    return stored ? this.getOrCreate(stored) : this.getOrCreate();
  }

  /**
   * Acquires the shopper session and an active cart ahead of the first
   * mutation, so the "add to cart" click is a single `PUT`. Call it on page
   * load without blocking render (don't `await` it on the critical path);
   * concurrent calls share one round of work. If it fails (e.g. offline) the
   * later mutation just pays the original session/create cost — nothing is
   * left half-initialised.
   */
  warmup(): Promise<DataEnvelope<Cart>> {
    if (!this.pendingWarmup) {
      this.pendingWarmup = (async () => {
        await this.client.acquireShopperToken();
        return this.resume();
      })().finally(() => {
        this.pendingWarmup = null;
      });
    }
    return this.pendingWarmup;
  }

  async setLine(
    cartId: string,
    productVariantId: string,
    body: SetCartLineRequest,
  ): Promise<DataEnvelope<Cart>> {
    return this.enqueueMutation(cartId, () =>
      this.mutateActiveCart(cartId, (activeCartId, activeCart) =>
        this.setLineRequest(activeCartId, productVariantId, body, activeCart),
      ),
    );
  }

  /** Adds a quantity to a Cart line. */
  async addLine(
    cartId: string,
    productVariantId: string,
    quantity = 1,
  ): Promise<DataEnvelope<Cart>> {
    if (!Number.isInteger(quantity) || quantity < 1) {
      throw new RangeError("quantity must be a positive integer");
    }
    return this.enqueueMutation(cartId, () =>
      this.mutateActiveCart(cartId, (activeCartId, activeCart) => {
        const existing = activeCart.lines.find(
          (line) => line.product_variant_id === productVariantId,
        );
        return this.setLineRequest(
          activeCartId,
          productVariantId,
          { quantity: (existing?.quantity ?? 0) + quantity },
          activeCart,
        );
      }),
    );
  }

  removeLine(
    cartId: string,
    productVariantId: string,
  ): Promise<DataEnvelope<Cart>> {
    return this.enqueueMutation(cartId, () =>
      this.mutateActiveCart(cartId, async (activeCartId, activeCart) => {
        const previousQuantity = activeCart.lines.find(
          (line) => line.product_variant_id === productVariantId,
        )?.quantity;
        const response = await this.client.request<DataEnvelope<Cart>>(
          `/carts/${encodeURIComponent(activeCartId)}/lines/${encodeURIComponent(productVariantId)}`,
          {
            method: "DELETE",
            requiresShopperToken: true,
          },
        );
        this.remember(response.data);
        this.client.recordCartMutation({
          cart: response.data,
          product_variant_id: productVariantId,
          previous_quantity: previousQuantity ?? 0,
          new_quantity: 0,
          removed: true,
        });
        return response;
      }),
    );
  }

  /**
   * Runs a line mutation against the current cart body, and once against a
   * freshly resolved active cart if the server rejects the first attempt with
   * `cart_not_active` (the cart was locked or completed by a checkout). A
   * recovered call returns a cart with a different `id` — callers that hold a
   * cart id must read it back from the response.
   */
  private async mutateActiveCart(
    cartId: string,
    perform: (
      activeCartId: string,
      activeCart: Cart,
    ) => Promise<DataEnvelope<Cart>>,
  ): Promise<DataEnvelope<Cart>> {
    try {
      return await perform(cartId, await this.snapshot(cartId));
    } catch (error) {
      if (!isCartNotActive(error)) throw error;
      this.forget(cartId);
      // `POST /carts` returns this shopper's canonical active cart, minting
      // one because the previous cart is no longer active.
      const fresh = await this.create();
      return perform(fresh.data.id, fresh.data);
    }
  }

  private async setLineRequest(
    cartId: string,
    productVariantId: string,
    body: SetCartLineRequest,
    previousCart: Cart,
  ): Promise<DataEnvelope<Cart>> {
    const previousQuantity = previousCart.lines.find(
      (line) => line.product_variant_id === productVariantId,
    )?.quantity;
    // Attach ad-platform attribution only when this raises the quantity —
    // the server emits AddToCart (to Meta CAPI) only then, and there is no
    // reason to ship Meta cookies on a decrement or a no-op.
    const increasing = body.quantity > (previousQuantity ?? 0);
    const response = await this.client.request<DataEnvelope<Cart>>(
      `/carts/${encodeURIComponent(cartId)}/lines/${encodeURIComponent(productVariantId)}`,
      {
        method: "PUT",
        body: increasing ? { ...body, ...adAttributionBody() } : body,
        requiresShopperToken: true,
      },
    );
    this.remember(response.data);
    const newQuantity = response.data.lines.find(
      (line) => line.product_variant_id === productVariantId,
    )?.quantity ?? 0;
    this.client.recordCartMutation({
      cart: response.data,
      product_variant_id: productVariantId,
      previous_quantity: previousQuantity ?? 0,
      new_quantity: newQuantity,
      removed: newQuantity === 0,
      // The server minted this AddToCart event id when the quantity rose; the
      // Pixel projection reuses it so Meta deduplicates the CAPI + Pixel copy.
      ...(response.data.event_id
        ? { event_id: response.data.event_id }
        : {}),
    });
    return response;
  }

  /**
   * Runs an operation serialized against this cart's mutation queue, so a
   * read used to build a request (e.g. checkout) cannot race a concurrent
   * line mutation for the same cart.
   * @internal
   */
  runExclusive<T>(cartId: string, operation: () => Promise<T>): Promise<T> {
    return this.enqueueMutation(cartId, operation);
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
