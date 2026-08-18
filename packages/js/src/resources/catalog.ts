import type { ChaosStorefrontClient } from "../client.js";
import type { Collection, CursorPageParams, DataEnvelope, PageEnvelope, Product, ResolvedStoreDomain } from "../types.js";

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

  /**
   * Resolves the active Store and Sales Channel from the request's Host
   * header. Browsers send this automatically for the current origin, so
   * call this with no arguments client-side. Server-side callers (SSR,
   * edge functions) generally cannot use this endpoint at all: it only
   * reads the actual HTTP Host header of the incoming request, and most
   * fetch implementations (including Node's) silently ignore an
   * application-set Host header on outbound requests rather than
   * forwarding it — there is no supported way to drive this endpoint from
   * a server issuing its own outbound call. A server-rendered storefront
   * should determine its Store from the publishable key instead.
   */
  resolveDomainContext(): Promise<DataEnvelope<ResolvedStoreDomain>> {
    return this.client.request("/domain-context", { method: "GET" });
  }

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
