import { apiErrorFromResponse } from "../errors.js";

export interface TransportRequestOptions<
  Query extends object = Record<string, never>,
> {
  method?: "GET" | "POST" | "PUT" | "DELETE";
  query?: Query;
  body?: unknown;
  requestId?: string;
  idempotencyKey?: string;
  signal?: AbortSignal;
}

interface TransportOptions {
  publishableKey: string;
  baseUrl: string;
  fetch?: typeof fetch;
}

/** HTTP serialization and error decoding for the Storefront wire protocol. */
export class StorefrontTransport {
  private readonly fetchImpl: typeof fetch;

  constructor(private readonly options: TransportOptions) {
    const fetchImpl = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (!fetchImpl) {
      throw new TypeError("window.fetch is required");
    }
    this.fetchImpl = fetchImpl;
  }

  async request<T, Query extends object = Record<string, never>>(
    path: string,
    options: TransportRequestOptions<Query> = {},
    shopperToken?: string,
  ): Promise<T> {
    const headers: Record<string, string> = {
      "X-Chaos-Publishable-Key": this.options.publishableKey,
    };
    if (options.body !== undefined) headers["content-type"] = "application/json";
    if (options.requestId) headers["X-Request-ID"] = options.requestId;
    if (options.idempotencyKey) headers["Idempotency-Key"] = options.idempotencyKey;
    if (shopperToken) headers["X-Chaos-Shopper-Token"] = shopperToken;

    const init: RequestInit = {
      method: options.method ?? "GET",
      headers,
      ...(options.signal ? { signal: options.signal } : {}),
    };
    if (options.body !== undefined) init.body = JSON.stringify(options.body);

    const response = await this.fetchImpl(
      buildUrl(this.options.baseUrl, path, options.query ?? {}),
      init,
    );
    if (!response.ok) throw await apiErrorFromResponse(response);
    if (
      response.status === 204 ||
      response.headers.get("content-length") === "0"
    ) {
      return undefined as T;
    }
    return (await response.json()) as T;
  }
}

function buildUrl(baseUrl: string, path: string, query: object): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== null) search.set(key, String(value));
  }
  const queryString = search.toString();

  const url = new URL(`${baseUrl}${path}`, window.location.origin);
  url.search = queryString;
  return url.toString();
}
