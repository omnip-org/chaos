import { ChaosStorefrontAnalytics, type AnalyticsOptions } from "./analytics.js";
import { throwForResponse } from "./errors.js";
import { CartResource } from "./resources/cart.js";
import { CatalogResource } from "./resources/catalog.js";
import { OrdersResource } from "./resources/orders.js";
import { PaymentsResource } from "./resources/payments.js";
import { ShopperSessionResource } from "./resources/shopper-session.js";
import type { ShopperSession } from "./types.js";

const SHOPPER_TOKEN_STORAGE_PREFIX = "chaos.storefront.shopper_token";

export interface ClientOptions {
  publishableKey: string;
  /** Storefront API origin + prefix, e.g. "https://shop.example.com/storefront/v1". Defaults to same-origin "/storefront/v1". */
  baseUrl?: string;
  fetch?: typeof fetch;
  /** Where the shopper token is persisted between requests. Defaults to window.localStorage when available. */
  storage?: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
  randomUUID?: () => string;
  /**
   * Options forwarded to the bundled analytics collector, minus
   * publishableKey/fetch/randomUUID (inherited from this client). Pass
   * `analytics: false` to skip constructing it entirely.
   */
  analytics?: Omit<AnalyticsOptions, "publishableKey" | "fetch" | "randomUUID" | "getShopperToken"> | false;
}

export interface RequestOptions<Query extends object = Record<string, never>> {
  method?: "GET" | "POST" | "PUT" | "DELETE";
  query?: Query;
  body?: unknown;
  /** Attaches the shopper token, acquiring one if the browser has not created a Shopper session yet. */
  requiresShopperToken?: boolean;
  /** Client-generated request identifier for order/checkout retries. */
  requestId?: string;
}

export class ChaosStorefrontClient {
  readonly publishableKey: string;
  readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
  private readonly shopperTokenStorageKey: string;
  readonly randomUUID: () => string;
  private shopperTokenCache: string | null = null;
  private pendingShopperSession: Promise<string> | null = null;

  readonly catalog: CatalogResource;
  readonly shopperSession: ShopperSessionResource;
  readonly cart: CartResource;
  readonly orders: OrdersResource;
  readonly payments: PaymentsResource;
  readonly analytics?: ChaosStorefrontAnalytics;

  constructor(options: ClientOptions) {
    if (!options.publishableKey) {
      throw new TypeError("publishableKey is required");
    }
    this.publishableKey = options.publishableKey;
    this.baseUrl = (options.baseUrl ?? "/storefront/v1").replace(/\/+$/, "");
    this.fetchImpl = options.fetch ?? globalThis.fetch?.bind(globalThis);
    this.storage = options.storage !== undefined ? options.storage : (globalThis.localStorage ?? null);
    this.shopperTokenStorageKey = scopedShopperTokenKey(this.baseUrl, this.publishableKey);
    this.randomUUID = options.randomUUID ?? globalThis.crypto?.randomUUID.bind(globalThis.crypto);
    if (!this.fetchImpl) {
      throw new TypeError("fetch is required (pass options.fetch in environments without a global fetch)");
    }
    if (!this.randomUUID) {
      throw new TypeError("randomUUID is required (pass options.randomUUID in environments without globalThis.crypto)");
    }
    try {
      this.shopperTokenCache = this.storage?.getItem(this.shopperTokenStorageKey) ?? null;
    } catch {
      this.shopperTokenCache = null;
    }

    if (globalThis.document) {
      void this.ensureShopperToken().catch(() => {
        // A transient identity request must not prevent catalog rendering;
        // the next shopper-scoped request retries acquisition.
      });
    }

    this.catalog = new CatalogResource(this);
    this.shopperSession = new ShopperSessionResource(this);
    this.cart = new CartResource(this);
    this.orders = new OrdersResource(this);
    this.payments = new PaymentsResource(this);
    const analyticsOptions = options.analytics === false ? undefined : options.analytics;
    const analyticsDocument = analyticsOptions?.document ?? globalThis.document;
    if (options.analytics !== false && analyticsDocument) {
      this.analytics = new ChaosStorefrontAnalytics({
        ...analyticsOptions,
        document: analyticsDocument,
        endpoint: analyticsOptions?.endpoint ?? `${this.baseUrl}/analytics/events`,
        publishableKey: this.publishableKey,
        fetch: this.fetchImpl,
        randomUUID: this.randomUUID,
        getShopperToken: () => this.ensureShopperToken(),
      });
    }
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

  private async ensureShopperToken(): Promise<string> {
    if (this.shopperTokenCache) return this.shopperTokenCache;
    if (!this.pendingShopperSession) {
      this.pendingShopperSession = this.createShopperSession().finally(() => {
        this.pendingShopperSession = null;
      });
    }
    return this.pendingShopperSession;
  }

  private async createShopperSession(): Promise<string> {
    const envelope = await this.request<{ data: ShopperSession }>("/shopper-sessions", { method: "POST" });
    this.setShopperToken(envelope.data.shopper_token);
    return envelope.data.shopper_token;
  }

  async request<T, Query extends object = Record<string, never>>(
    path: string,
    options: RequestOptions<Query> = {},
  ): Promise<T> {
    return this.requestWithShopperTokenRetry(path, options, true);
  }

  private async requestWithShopperTokenRetry<T, Query extends object = Record<string, never>>(
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
    if (response.status === 204 || response.headers.get("content-length") === "0") {
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
      const url = new URL(`${this.baseUrl}${path}`, isAbsolute ? undefined : origin);
      url.search = queryString;
      return url.toString();
    }

    // No absolute baseUrl and no global `location` (e.g. Node/SSR without an
    // explicit origin): fall back to a path-only URL string, which fetch
    // implementations resolve against their own base.
    return queryString ? `${this.baseUrl}${path}?${queryString}` : `${this.baseUrl}${path}`;
  }
}

export function createStorefrontClient(options: ClientOptions): ChaosStorefrontClient {
  return new ChaosStorefrontClient(options);
}

function scopedShopperTokenKey(baseUrl: string, publishableKey: string): string {
  let hash = 2_166_136_261;
  const input = `${baseUrl}\0${publishableKey}`;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
  }
  return `${SHOPPER_TOKEN_STORAGE_PREFIX}.${(hash >>> 0).toString(36)}`;
}
