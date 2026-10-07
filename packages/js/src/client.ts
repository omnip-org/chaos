import { ChaosApiError } from "./errors.js";
import type { AnalyticsOptions } from "./events/browser.js";
import { StorefrontEventCoordinator } from "./internal/storefront-events.js";
import { ShopperSessionStore } from "./internal/shopper-session.js";
import { StorefrontTransport } from "./internal/transport.js";
import { CartResource } from "./resources/cart.js";
import { CatalogResource } from "./resources/catalog.js";
import { OrdersResource } from "./resources/orders.js";
import { PaymentsResource } from "./resources/payments.js";
import { ReviewsResource } from "./resources/reviews.js";
import {
  ShopperSessionResource,
  requireShopperSession,
} from "./resources/shopper-session.js";
import type {
  CartLineMutation,
  ConfirmedPurchaseOrderInput,
  CheckoutUtm,
  DataEnvelope,
  EmbeddedCheckoutCreation,
  EmbeddedCheckoutStart,
  OwnOrder,
  Product,
  ShopperSession,
} from "./types.js";

const DEFAULT_CART_SNAPSHOT_TTL_MS = 30_000;

export type StorefrontEventsOptions = AnalyticsOptions;

export interface ClientOptions {
  publishableKey: string;
  /** Chaos API origin + prefix, e.g. "https://chaos.example.com/api/v1". */
  baseUrl?: string;
  fetch?: typeof fetch;
  /**
   * Where the shopper token is persisted between requests. Defaults to
   * window.localStorage; pass null for an in-memory browser session.
   */
  storage?: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
  randomUUID?: () => string;
  /**
   * Disables implicit shopper-session creation for callers that need to
   * distinguish a missing token from a new anonymous session. Defaults to true.
   */
  autoAcquireShopperToken?: boolean;
  /**
   * Retries one request rejected with `shopper_token_invalid` using a newly
   * issued token. This is opt-in because changing shopper identity can orphan
   * a cart or hide an order; use CartResource.getOrCreate for explicit cart
   * recovery.
   */
  retryInvalidShopperToken?: boolean;
  /** Turns on client-side Meta Pixel/GA4 event delivery; omit to leave it off. */
  events?: StorefrontEventsOptions;
  /**
   * How long (ms) a cart body returned by the API is reused to serve the read
   * that `cart.addLine`/`setLine`/`removeLine` do before their write, instead
   * of a separate `GET /carts/{id}`. 0 disables it. Defaults to 30000.
   */
  cartSnapshotTtlMs?: number;
  /** Millisecond clock used for cart snapshots and attribution timestamps. */
  now?: () => number;
}

export interface RequestOptions<
  Query extends object = Record<string, never>,
> {
  method?: "GET" | "POST" | "PUT" | "DELETE";
  query?: Query;
  body?: unknown;
  /** Attaches the shopper token, acquiring one when configured to do so. */
  requiresShopperToken?: boolean;
  /** Override the client's invalid-token retry for a specific request. */
  retryShopperToken?: boolean;
  /** Optional trace identifier propagated as X-Request-ID. */
  requestId?: string;
  /** Business idempotency key sent as Idempotency-Key. */
  idempotencyKey?: string;
  /** Cancels the underlying fetch. */
  signal?: AbortSignal;
}

/** Public Storefront facade; protocol, session and event state live internally. */
export class ChaosStorefrontClient {
  /** @internal */
  readonly publishableKey: string;
  /** @internal */
  readonly baseUrl: string;
  /** @internal */
  readonly randomUUID: () => string;
  /** @internal */
  readonly now: () => number;

  private readonly transport: StorefrontTransport;
  private readonly sessions: ShopperSessionStore;
  private readonly events: StorefrontEventCoordinator;
  private readonly retryInvalidShopperToken: boolean;

  readonly catalog: CatalogResource;
  readonly shopperSession: ShopperSessionResource;
  readonly cart: CartResource;
  readonly orders: OrdersResource;
  readonly payments: PaymentsResource;
  readonly reviews: ReviewsResource;

  constructor(options: ClientOptions) {
    assertBrowserRuntime();
    if (!options.publishableKey) throw new TypeError("publishableKey is required");

    this.publishableKey = options.publishableKey;
    this.baseUrl = (options.baseUrl ?? "/api/v1").replace(/\/+$/, "");
    this.randomUUID =
      options.randomUUID ?? globalThis.crypto?.randomUUID.bind(globalThis.crypto);
    this.now = options.now ?? (() => Date.now());
    if (!this.randomUUID) {
      throw new TypeError(
        "randomUUID is required (pass options.randomUUID in environments " +
          "without globalThis.crypto)",
      );
    }

    this.transport = new StorefrontTransport({
      publishableKey: this.publishableKey,
      baseUrl: this.baseUrl,
      ...(options.fetch ? { fetch: options.fetch } : {}),
    });
    this.events = new StorefrontEventCoordinator({
      now: this.now,
      randomUUID: this.randomUUID,
      ...(options.events ? { events: options.events } : {}),
    });
    this.sessions = new ShopperSessionStore({
      publishableKey: this.publishableKey,
      baseUrl: this.baseUrl,
      autoAcquire: options.autoAcquireShopperToken ?? true,
      ...(options.storage !== undefined ? { storage: options.storage } : {}),
      setAnalyticsShopperId: (shopperId) => this.events.setShopperId(shopperId),
      clearAnalyticsShopperId: () => this.events.clearShopperId(),
    });
    this.retryInvalidShopperToken =
      options.retryInvalidShopperToken ?? false;

    this.catalog = new CatalogResource(this);
    this.shopperSession = new ShopperSessionResource(this);
    this.cart = new CartResource(
      this,
      options.cartSnapshotTtlMs ?? DEFAULT_CART_SNAPSHOT_TTL_MS,
    );
    this.orders = new OrdersResource(this);
    this.payments = new PaymentsResource(this);
    this.reviews = new ReviewsResource(this);
  }

  getShopperToken(): string | null {
    return this.sessions.token;
  }

  setShopperToken(token: string | null): void {
    this.sessions.setToken(token);
  }

  /** @internal First-touch attribution used when creating a shopper session. */
  firstTouchUtm(): CheckoutUtm | undefined {
    return this.sessions.firstTouchUtm();
  }

  /** Acquires one shopper session; concurrent callers share the request. */
  async acquireShopperToken(): Promise<string> {
    return this.sessions.acquire(() => this.issueShopperSession());
  }

  /** @internal Creates, validates and installs a new shopper identity. */
  async issueShopperSession(): Promise<DataEnvelope<ShopperSession>> {
    const utm = this.firstTouchUtm();
    const response = await this.transport.request<unknown>(
      "/shopper/sessions",
      {
        method: "POST",
        body: utm ? { attribution: { utm } } : {},
      },
    );
    const envelope = requireShopperSession(response);
    this.sessions.install(envelope.data);
    return envelope;
  }

  /** @internal Whether this client created the current shopper identity. */
  get shopperSessionWasMintedHere(): boolean {
    return this.sessions.wasMintedHere;
  }

  /** Low-level Storefront request entry point used by the typed resources. */
  async request<T, Query extends object = Record<string, never>>(
    path: string,
    options: RequestOptions<Query> = {},
  ): Promise<T> {
    return this.requestWithShopperTokenRetry(
      path,
      options,
      options.retryShopperToken ?? this.retryInvalidShopperToken,
    );
  }

  /** @internal Used by CartResource after a successful line mutation. */
  recordCartMutation(mutation: CartLineMutation): void {
    this.events.recordCartMutation(mutation);
  }

  /** @internal Used by PaymentsResource after checkout creation. */
  recordCheckoutCreation(
    creation: EmbeddedCheckoutStart | EmbeddedCheckoutCreation,
  ): void {
    this.events.recordCheckoutCreation(creation);
  }

  /** @internal Called only after a shopper-owned Order read. */
  async recordCheckoutPurchase(order: OwnOrder): Promise<void> {
    await this.events.recordCheckoutPurchase(order, (confirmed) =>
      this.recordConfirmedPurchase(confirmed),
    );
  }

  /** @internal Projects a server-confirmed Order to configured browser providers. */
  recordConfirmedPurchase(order: ConfirmedPurchaseOrderInput): void {
    this.events.recordConfirmedPurchase(order);
  }

  /** @internal Projects a storefront search to configured browser providers. */
  recordSearch(input: { query: string }): void {
    this.events.recordSearch(input);
  }

  /** @internal Used after a Product detail page has been opened. */
  recordProductView(product: Product): void {
    this.events.recordProductView(product);
  }

  private async requestWithShopperTokenRetry<
    T,
    Query extends object = Record<string, never>,
  >(
    path: string,
    options: RequestOptions<Query>,
    retryShopperToken: boolean,
  ): Promise<T> {
    const shopperToken = options.requiresShopperToken
      ? await this.sessions.require(() => this.issueShopperSession())
      : undefined;
    try {
      return await this.transport.request<T, Query>(
        path,
        options,
        shopperToken,
      );
    } catch (error) {
      if (
        retryShopperToken &&
        options.requiresShopperToken &&
        error instanceof ChaosApiError &&
        error.status === 401 &&
        error.code === "shopper_token_invalid" &&
        this.sessions.token
      ) {
        this.sessions.setToken(null);
        return this.requestWithShopperTokenRetry(path, options, false);
      }
      throw error;
    }
  }
}

function assertBrowserRuntime(): void {
  if (typeof window === "undefined" || typeof document === "undefined") {
    throw new TypeError(
      "@omnip-org/chaos-js is browser-only; construct ChaosStorefrontClient in a browser",
    );
  }
}
