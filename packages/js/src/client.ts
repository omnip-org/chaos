import { ChaosApiError, throwForResponse } from "./errors.js";
import {
  ChaosStorefrontAnalytics,
  type AnalyticsOptions,
} from "./events/browser.js";
import { fnv1a32 } from "./internal/hash.js";
import { CartResource } from "./resources/cart.js";
import { CatalogResource } from "./resources/catalog.js";
import { OrdersResource } from "./resources/orders.js";
import { PaymentsResource } from "./resources/payments.js";
import { ReviewsResource } from "./resources/reviews.js";
import { ShopperSessionResource } from "./resources/shopper-session.js";
import type {
  CartLineMutation,
  EmbeddedCheckoutCreation,
  OrderLookup,
  ShopperSession,
} from "./types.js";

const SHOPPER_TOKEN_STORAGE_PREFIX = "chaos.storefront.shopper_token";

/**
 * Meta Pixel/GA4 event delivery, keyed by destination: pass `metaPixel` to
 * turn on Pixel, `ga4` to turn on GA4, omit either to leave it off — there
 * is no separate enable flag.
 */
export type StorefrontEventsOptions = Omit<AnalyticsOptions, "publishableKey">;

export interface ClientOptions {
  publishableKey: string;
  /** Chaos API origin + prefix, e.g. "https://chaos.example.com/api/v1". */
  baseUrl?: string;
  fetch?: typeof fetch;
  /** Where the shopper token is persisted between requests. Defaults to window.localStorage when available. */
  storage?: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
  randomUUID?: () => string;
  /**
   * Disables implicit shopper-session creation for callers that need to
   * distinguish a missing token from a new anonymous session. Defaults to
   * true for browser compatibility.
   */
  autoAcquireShopperToken?: boolean;
  /**
   * Retries one 401/403 shopper request with a newly issued token. This is
   * opt-in because changing shopper identity can orphan a cart or hide an
   * order; use CartResource.getOrCreate for explicit cart recovery.
   */
  retryInvalidShopperToken?: boolean;
  /** Turns on client-side Meta Pixel/GA4 event delivery; omit to leave it off. */
  events?: StorefrontEventsOptions;
}

/** @internal */
export interface RequestOptions<Query extends object = Record<string, never>> {
  method?: "GET" | "POST" | "PUT" | "DELETE";
  query?: Query;
  body?: unknown;
  /** Attaches the shopper token, acquiring one if the browser has not created a Shopper session yet. */
  requiresShopperToken?: boolean;
  /** Optional trace identifier propagated as X-Request-ID. */
  requestId?: string;
  /** Business idempotency key sent as Idempotency-Key. */
  idempotencyKey?: string;
  /** Optimistic concurrency validator sent as If-Match. */
  ifMatch?: string;
}

export class ChaosStorefrontClient {
  readonly publishableKey: string;
  readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly storage: Pick<
    Storage,
    "getItem" | "setItem" | "removeItem"
  > | null;
  private readonly shopperTokenStorageKey: string;
  private readonly autoAcquireShopperToken: boolean;
  private readonly retryInvalidShopperToken: boolean;
  private readonly analytics: ChaosStorefrontAnalytics | null;
  readonly randomUUID: () => string;
  private shopperTokenCache: string | null = null;
  private pendingShopperSession: Promise<string> | null = null;

  readonly catalog: CatalogResource;
  readonly shopperSession: ShopperSessionResource;
  readonly cart: CartResource;
  readonly orders: OrdersResource;
  readonly payments: PaymentsResource;
  readonly reviews: ReviewsResource;

  constructor(options: ClientOptions) {
    if (!options.publishableKey) {
      throw new TypeError("publishableKey is required");
    }
    this.publishableKey = options.publishableKey;
    this.baseUrl = (options.baseUrl ?? "/api/v1").replace(/\/+$/, "");
    this.fetchImpl = options.fetch ?? globalThis.fetch?.bind(globalThis);
    this.storage =
      options.storage !== undefined
        ? options.storage
        : (globalThis.localStorage ?? null);
    this.shopperTokenStorageKey = scopedShopperTokenKey(
      this.baseUrl,
      this.publishableKey,
    );
    this.autoAcquireShopperToken = options.autoAcquireShopperToken ?? true;
    this.retryInvalidShopperToken = options.retryInvalidShopperToken ?? false;
    this.analytics = options.events
      ? new ChaosStorefrontAnalytics({
          // `publishableKey` here only namespaces the analytics module's own
          // local-storage keys; it doesn't need to be a real Chaos key.
          publishableKey: `${this.baseUrl}\0${this.publishableKey}`,
          ...options.events,
        })
      : null;
    this.randomUUID =
      options.randomUUID ??
      globalThis.crypto?.randomUUID.bind(globalThis.crypto);
    if (!this.fetchImpl) {
      throw new TypeError(
        "fetch is required (pass options.fetch in environments without a global fetch)",
      );
    }
    if (!this.randomUUID) {
      throw new TypeError(
        "randomUUID is required (pass options.randomUUID in environments without globalThis.crypto)",
      );
    }
    try {
      this.shopperTokenCache =
        this.storage?.getItem(this.shopperTokenStorageKey) ?? null;
    } catch {
      this.shopperTokenCache = null;
    }

    this.catalog = new CatalogResource(this);
    this.shopperSession = new ShopperSessionResource(this);
    this.cart = new CartResource(this);
    this.orders = new OrdersResource(this);
    this.payments = new PaymentsResource(this);
    this.reviews = new ReviewsResource(this);
  }

  getShopperToken(): string | null {
    return this.shopperTokenCache;
  }

  setShopperToken(token: string | null): void {
    this.shopperTokenCache = token;
    try {
      if (token) {
        this.storage?.setItem(this.shopperTokenStorageKey, token);
      } else {
        this.storage?.removeItem(this.shopperTokenStorageKey);
      }
    } catch {
      // Storage is optional; the in-memory token remains usable.
    }
  }

  /**
   * Explicitly acquires a shopper session when one is not already cached.
   * Concurrent callers share the same in-flight request.
   */
  async acquireShopperToken(): Promise<string> {
    if (this.shopperTokenCache) return this.shopperTokenCache;
    if (!this.pendingShopperSession) {
      this.pendingShopperSession = this.createShopperSession().finally(() => {
        this.pendingShopperSession = null;
      });
    }
    return this.pendingShopperSession;
  }

  private async ensureShopperToken(): Promise<string> {
    if (this.shopperTokenCache) return this.shopperTokenCache;
    if (!this.autoAcquireShopperToken) {
      throw new ChaosApiError(
        401,
        "shopper_token_required",
        "a shopper token is required for this request",
      );
    }
    return this.acquireShopperToken();
  }

  private async createShopperSession(): Promise<string> {
    const envelope = await this.request<{ data: ShopperSession }>(
      "/shopper/sessions",
      { method: "POST" },
    );
    this.setShopperToken(envelope.data.shopper_token);
    return envelope.data.shopper_token;
  }

  /** @internal */
  async request<T, Query extends object = Record<string, never>>(
    path: string,
    options: RequestOptions<Query> = {},
  ): Promise<T> {
    return this.requestWithShopperTokenRetry(
      path,
      options,
      this.retryInvalidShopperToken,
    );
  }

  /** @internal Used by CartResource after a successful line mutation. */
  recordCartMutation(mutation: CartLineMutation): void {
    try {
      this.analytics?.recordCartMutation(mutation);
    } catch {
      // The cart mutation already succeeded; analytics must remain best-effort.
    }
  }

  /** @internal Used by PaymentsResource after a successful checkout creation. */
  recordCheckoutCreation(creation: EmbeddedCheckoutCreation): void {
    try {
      this.analytics?.recordCheckoutCreation(creation);
    } catch {
      // The checkout already exists; analytics must remain best-effort.
    }
  }

  /**
   * Projects a confirmed, paid order to Meta Pixel/GA4 — never inferred from
   * browser activity. Typically called on a return page right after
   * `orders.lookupOrder`. No-op unless the order is confirmed and paid.
   */
  recordConfirmedPurchase(
    order: Pick<
      OrderLookup,
      "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
    >,
  ): void {
    try {
      this.analytics?.recordConfirmedPurchase(order);
    } catch {
      // The order is already confirmed; analytics must remain best-effort.
    }
  }

  /** @internal Used by CatalogResource.listProducts when `q` is set. */
  recordSearch(input: { query: string }): void {
    try {
      this.analytics?.search(input);
    } catch {
      // The search already ran; analytics must remain best-effort.
    }
  }

  /** @internal Used by CatalogResource.getProduct. */
  recordViewContent(input: { productId: string; productVariantId?: string }): void {
    try {
      this.analytics?.viewContent(input);
    } catch {
      // The product already loaded; analytics must remain best-effort.
    }
  }

  private async requestWithShopperTokenRetry<
    T,
    Query extends object = Record<string, never>,
  >(
    path: string,
    options: RequestOptions<Query>,
    retryShopperToken: boolean,
  ): Promise<T> {
    const method = options.method ?? "GET";
    const headers: Record<string, string> = {
      authorization: `Bearer ${this.publishableKey}`,
    };

    if (options.body !== undefined) {
      headers["content-type"] = "application/json";
    }
    if (options.requestId) {
      headers["X-Request-ID"] = options.requestId;
    }
    if (options.idempotencyKey) {
      headers["Idempotency-Key"] = options.idempotencyKey;
    }
    if (options.ifMatch) {
      headers["If-Match"] = options.ifMatch;
    }
    if (options.requiresShopperToken) {
      headers["x-chaos-shopper-token"] = await this.ensureShopperToken();
    }
    const requestUrl = this.buildUrl(path, options.query ?? {});

    const init: RequestInit = { method, headers };
    if (options.body !== undefined) {
      init.body = JSON.stringify(options.body);
    }
    const response = await this.fetchImpl(requestUrl, init);

    if (
      !response.ok &&
      retryShopperToken &&
      options.requiresShopperToken &&
      (response.status === 401 || response.status === 403) &&
      this.shopperTokenCache
    ) {
      this.setShopperToken(null);
      return this.requestWithShopperTokenRetry(path, options, false);
    }
    if (!response.ok) {
      await throwForResponse(response);
    }
    if (
      response.status === 204 ||
      response.headers.get("content-length") === "0"
    ) {
      return undefined as T;
    }
    return (await response.json()) as T;
  }

  private buildUrl(path: string, query: object): string {
    const origin = globalThis.location?.origin;
    const isAbsolute = /^https?:\/\//.test(this.baseUrl);
    const search = new URLSearchParams();
    for (const [key, value] of Object.entries(query)) {
      if (value !== undefined && value !== null) search.set(key, String(value));
    }
    const queryString = search.toString();

    if (isAbsolute || origin) {
      const url = new URL(
        `${this.baseUrl}${path}`,
        isAbsolute ? undefined : origin,
      );
      url.search = queryString;
      return url.toString();
    }

    // No absolute baseUrl and no global `location` (e.g. Node/SSR without an
    // explicit origin): fall back to a path-only URL string, which fetch
    // implementations resolve against their own base.
    return queryString
      ? `${this.baseUrl}${path}?${queryString}`
      : `${this.baseUrl}${path}`;
  }
}

function scopedShopperTokenKey(
  baseUrl: string,
  publishableKey: string,
): string {
  const hash = fnv1a32(`${baseUrl}\0${publishableKey}`);
  return `${SHOPPER_TOKEN_STORAGE_PREFIX}.${hash.toString(36)}`;
}
