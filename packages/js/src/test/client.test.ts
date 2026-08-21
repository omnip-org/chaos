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

test("does not construct browser analytics during SSR", () => {
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    baseUrl: "https://shop.example.com/store/v1",
    storage: null,
    randomUUID: () => "idempotency-key",
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });

  assert.equal(client.analytics, undefined);
});

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
  assert.equal(client.getShopperToken(), "shopper-token-abc");
});

test("reuses a shopper token persisted from a previous session", async () => {
  const storage = new MemoryStorage();
  const firstClient = createStorefrontClient({
    publishableKey: "pk_test",
    storage,
    analytics: false,
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });
  firstClient.setShopperToken("existing-token");
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

test("explicit shopper sessions update the client token", async () => {
  const storage = new MemoryStorage();
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    storage,
    analytics: false,
    fetch: (async () => jsonResponse(201, { data: { shopper_token: "manual-token" } })) as unknown as typeof fetch,
  });

  await client.shopperSession.create();

  assert.equal(client.getShopperToken(), "manual-token");
});

test("refreshes a stale shopper token once and retries the request", async () => {
  const storage = new MemoryStorage();
  const requests: Array<{ url: string; token: string | undefined }> = [];
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    storage,
    analytics: false,
    fetch: (async (url: string, init: RequestInit) => {
      const headers = new Headers(init.headers);
      requests.push({ url, token: headers.get("x-chaos-shopper-token") ?? undefined });
      if (url.endsWith("/carts/cart-1")) {
        if (requests.at(-1)?.token === "stale-token") return jsonResponse(401, { error: { code: "unauthorized" } });
        return jsonResponse(200, { data: { id: "cart-1", lines: [] } });
      }
      return jsonResponse(201, { data: { shopper_token: "fresh-token" } });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("stale-token");

  await client.cart.get("cart-1");

  assert.deepEqual(
    requests.map((request) => request.token),
    ["stale-token", undefined, "fresh-token"],
  );
  assert.equal(client.getShopperToken(), "fresh-token");
});

test("serializes concurrent addLine calls for one cart", async () => {
  let quantity = 1;
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    storage: null,
    analytics: false,
    randomUUID: () => "idempotency-key",
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper-sessions")) {
        return jsonResponse(201, { data: { shopper_token: "shopper-token" } });
      }
      if (init.method === "GET") {
        await new Promise((resolve) => setTimeout(resolve, 0));
        return jsonResponse(200, {
          data: { id: "cart-1", lines: [{ product_variant_id: "variant-1", quantity }] },
        });
      }
      quantity = JSON.parse(String(init.body)).quantity;
      return jsonResponse(200, {
        data: { id: "cart-1", lines: [{ product_variant_id: "variant-1", quantity }] },
      });
    }) as unknown as typeof fetch,
  });

  await Promise.all([
    client.cart.addLine("cart-1", "variant-1"),
    client.cart.addLine("cart-1", "variant-1"),
  ]);

  assert.equal(quantity, 3);
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

test("semantic SDK operations record conversion events only after successful responses", async () => {
  const recorded: Array<[string, unknown]> = [];
  const client = createStorefrontClient({
    publishableKey: "pk_test",
    storage: new MemoryStorage(),
    analytics: false,
    randomUUID: () => "idempotency-key",
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });
  // Replace transport and analytics at the resource boundary to test orchestration only.
  const mutable = client as unknown as {
    analytics: Record<string, (input: unknown) => void>;
    request: (path: string) => Promise<unknown>;
  };
  mutable.analytics = {
    search: (input) => recorded.push(["search", input]),
    viewContent: (input) => recorded.push(["view_content", input]),
    addToCart: (input) => recorded.push(["add_to_cart", input]),
    initiateCheckout: (input) => recorded.push(["initiate_checkout", input]),
  };
  mutable.request = async (path) => {
    if (path === "/products") return { data: [{ id: "product-1" }], meta: { page: { has_more: false } } };
    if (path.startsWith("/products/")) return { data: { id: "product-1" } };
    if (path.endsWith("/checkout")) return { data: { id: "checkout-1" } };
    if (path === "/carts/cart-1") return { data: { id: "cart-1", lines: [] } };
    return { data: { id: "cart-1", lines: [{ product_variant_id: "variant-1", quantity: 2 }] } };
  };

  await client.catalog.listProducts({ q: "shoes" });
  await client.catalog.getProduct("shoe");
  await client.cart.addLine("cart-1", "variant-1", 2);
  await client.checkout.create("cart-1", {
    contact: { email: "shopper@example.com" },
    billing_address: {
      full_name: "Shopper",
      address_line1: "1 Main Street",
      locality: "Singapore",
      country_code: "SG",
    },
  });

  assert.deepEqual(recorded, [
    ["search", { query: "shoes", resultCount: 1 }],
    ["view_content", { productId: "product-1" }],
    ["add_to_cart", { cartId: "cart-1", productVariantId: "variant-1", quantity: 2 }],
    ["initiate_checkout", { cartId: "cart-1", checkoutId: "checkout-1" }],
  ]);
});
