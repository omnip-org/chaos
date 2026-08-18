import type { ChaosStorefrontClient } from "../client.js";
import type { Collection, CursorPageParams, DataEnvelope, PageEnvelope, Product } from "../types.js";

export interface ListProductsParams extends CursorPageParams {
  currency?: string;
  /** Store-isolated full-text search. */
  q?: string;
  /** Active Collection handle; results preserve manual Collection order. */
  collection?: string;
  locale?: string;
}

export interface GetProductParams {
  currency?: string;
  locale?: string;
}

export interface ListCollectionsParams extends CursorPageParams {
  locale?: string;
}

export class CatalogResource {
  constructor(private readonly client: ChaosStorefrontClient) {}

  listProducts(params: ListProductsParams = {}): Promise<PageEnvelope<Product>> {
    return this.client.request("/products", { method: "GET", query: params });
  }

  getProduct(handle: string, params: GetProductParams = {}): Promise<DataEnvelope<Product>> {
    return this.client.request(`/products/${encodeURIComponent(handle)}`, { method: "GET", query: params });
  }

  listCollections(params: ListCollectionsParams = {}): Promise<PageEnvelope<Collection>> {
    return this.client.request("/collections", { method: "GET", query: params });
  }

  getCollection(handle: string, params: { locale?: string } = {}): Promise<DataEnvelope<Collection>> {
    return this.client.request(`/collections/${encodeURIComponent(handle)}`, { method: "GET", query: params });
  }
}
