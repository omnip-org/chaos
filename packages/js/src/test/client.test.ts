import assert from "node:assert/strict";
import test from "node:test";

import { createStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";

class MemoryStorage {
  private readonly values = new Map<string, string>();
  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
  removeItem(key: string): void {
    this.values.delete(key);
  }
}

function jsonResponse(status: number, body: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: new Headers({ "content-type": "application/json" }),
    json: async () => body,
  } as unknown as Response;
}

test("acquires a shopper session lazily on first cart mutation and reuses it", async () => {
  const requests: Array<{ url: string; headers: Record<string, string> }> = [];
  let sequence = 0;
  const storage = new MemoryStorage();
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    storage,
    randomUUID: () => `id-${++sequence}`,
    analytics: false,
    fetch: (async (url: string, init: RequestInit) => {
      const headers: Record<string, string> = {};
      new Headers(init.headers).forEach((value, key) => {
        headers[key] = value;
      });
      requests.push({ url: String(url), headers });
      if (String(url).endsWith("/shopper-sessions")) {
        return jsonResponse(201, { data: { shopper_token: "shopper-token-abc" } });
      }
      return jsonResponse(201, { data: { id: "cart-1", lines: [] } });
    }) as unknown as typeof fetch,
  });

  await client.cart.create();
  await client.cart.get("cart-1");

  assert.equal(requests.length, 3);
  assert.match(requests[0]!.url, /\/shopper-sessions$/);
  assert.equal(requests[1]!.headers["x-chaos-shopper-token"], "shopper-token-abc");
  assert.equal(requests[2]!.headers["x-chaos-shopper-token"], "shopper-token-abc");
  assert.equal(storage.getItem("chaos.storefront.shopper_token"), "shopper-token-abc");
});

test("reuses a shopper token persisted from a previous session", async () => {
  const storage = new MemoryStorage();
  storage.setItem("chaos.storefront.shopper_token", "existing-token");
  const requests: string[] = [];
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    storage,
    analytics: false,
    fetch: (async (url: string) => {
      requests.push(String(url));
      return jsonResponse(200, { data: { id: "cart-1", lines: [] } });
    }) as unknown as typeof fetch,
  });

  await client.cart.get("cart-1");

  assert.equal(requests.length, 1);
  assert.doesNotMatch(requests[0]!, /shopper-sessions/);
});

test("maps non-2xx responses to a typed ChaosApiError with server details", async () => {
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    storage: new MemoryStorage(),
    analytics: false,
    fetch: (async () =>
      jsonResponse(422, {
        error: {
          code: "validation_failed",
          message: "quantity must be at least 1",
          details: [{ field: "quantity", reason: "must be >= 1" }],
        },
      })) as unknown as typeof fetch,
  });

  await assert.rejects(
    client.catalog.listProducts(),
    (error: unknown) => {
      if (!(error instanceof ChaosApiError)) return false;
      assert.equal(error.status, 422);
      assert.equal(error.code, "validation_failed");
      assert.deepEqual(error.details, [{ field: "quantity", reason: "must be >= 1" }]);
      return true;
    },
  );
});

test("catalog.listProducts forwards query parameters", async () => {
  const captured: { url: URL | null } = { url: null };
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    baseUrl: "https://shop.example.com/store/v1",
    storage: new MemoryStorage(),
    analytics: false,
    fetch: (async (url: string) => {
      captured.url = new URL(String(url));
      return jsonResponse(200, { data: [], meta: { page: { has_more: false } } });
    }) as unknown as typeof fetch,
  });

  await client.catalog.listProducts({ q: "shoes", limit: 10, collection: "sale" });

  assert.equal(captured.url?.pathname, "/store/v1/products");
  assert.equal(captured.url?.searchParams.get("q"), "shoes");
  assert.equal(captured.url?.searchParams.get("limit"), "10");
  assert.equal(captured.url?.searchParams.get("collection"), "sale");
});
