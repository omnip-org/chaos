import type { ChaosStorefrontClient } from "../client.js";
import type { Collection, CursorPageParams, DataEnvelope, PageEnvelope, Product } from "../types.js";

const COLLECTION_CACHE_TTL_MS = 60_000;
const collectionCache = new Map<string, CollectionCacheEntry>();

interface CollectionCacheEntry {
  expiresAt: number;
  promise: Promise<Collection[]>;
}

export interface ListProductsParams extends CursorPageParams {
  currency?: string;
  /** Store-isolated full-text search. */
  q?: string;
  /** Active Collection handle; results preserve manual Collection order. */
  collection?: string;
}

export interface GetProductParams {
  currency?: string;
}

export type ListCollectionsParams = CursorPageParams;

export class CatalogResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  listProducts(params: ListProductsParams = {}): Promise<PageEnvelope<Product>> {
    return this.client.request<PageEnvelope<Product>, ListProductsParams>("/products", {
      method: "GET",
      query: params,
    });
  }

  getProduct(handle: string, params: GetProductParams = {}): Promise<DataEnvelope<Product>> {
    return this.client.request<DataEnvelope<Product>, GetProductParams>(
      `/products/${encodeURIComponent(handle)}`,
      { method: "GET", query: params },
    );
  }

  listCollections(params: ListCollectionsParams = {}): Promise<PageEnvelope<Collection>> {
    return this.client.request("/collections", { method: "GET", query: params });
  }

  /**
   * Returns the first collection page through a short, client-scoped cache.
   * The cache key includes both API origin and publishable key so a shared
   * Worker isolate can never reuse one store's navigation data for another.
   */
  listCollectionsCached(
    params: ListCollectionsParams = {},
    ttlMs = COLLECTION_CACHE_TTL_MS,
  ): Promise<Collection[]> {
    if (!Number.isFinite(ttlMs) || ttlMs < 0) {
      throw new RangeError("ttlMs must be a non-negative finite number");
    }
    const key = `${this.client.baseUrl}\0${this.client.publishableKey}\0${JSON.stringify(params)}`;
    const now = Date.now();
    pruneExpiredCollectionCache(now);
    const cached = collectionCache.get(key);
    if (cached && cached.expiresAt > now) return cached.promise;

    const promise = this.listCollections(params).then((response) => response.data);
    collectionCache.set(key, { expiresAt: now + ttlMs, promise });
    void promise.catch(() => {
      if (collectionCache.get(key)?.promise === promise) collectionCache.delete(key);
    });
    return promise;
  }

  getCollection(handle: string, params: Record<string, never> = {}): Promise<DataEnvelope<Collection>> {
    return this.client.request(`/collections/${encodeURIComponent(handle)}`, { method: "GET", query: params });
  }
}

/**
 * The cache is a module-level singleton so a shared Worker isolate keeps one
 * warm cache across per-request client instances (see `listCollectionsCached`).
 * Without this sweep, entries for stores/queries that stop being requested
 * would sit in memory forever once expired instead of being reclaimed.
 */
function pruneExpiredCollectionCache(now: number): void {
  for (const [key, entry] of collectionCache) {
    if (entry.expiresAt <= now) collectionCache.delete(key);
  }
}
