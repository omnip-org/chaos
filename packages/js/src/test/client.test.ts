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
    publishableKey: "public_test",
    baseUrl: "https://shop.example.com/storefront/v1",
    storage: null,
    randomUUID: () => "random-id",
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });

  assert.equal(client.analytics, undefined);
});

test("defers shopper session creation until a browser request needs it", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", { value: {}, configurable: true });
  const requests: string[] = [];
  try {
    const client = createStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      analytics: false,
      fetch: (async (url: string) => {
        requests.push(url);
        return jsonResponse(201, { data: { shopper_token: "browser-shopper-token" } });
      }) as unknown as typeof fetch,
    });

    await Promise.resolve();
    assert.equal(requests.length, 0);
    await client.cart.create();
    assert.equal(requests.length, 2);
    assert.match(requests[0]!, /\/shopper\/sessions$/);
  } finally {
    if (descriptor) {
      Object.defineProperty(globalThis, "document", descriptor);
    } else {
      Reflect.deleteProperty(globalThis, "document");
    }
  }
});

test("acquires a shopper session on the first shopper-scoped request and reuses it", async () => {
  const requests: Array<{ url: string; headers: Record<string, string> }> = [];
  let sequence = 0;
  const storage = new MemoryStorage();
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage,
    randomUUID: () => `id-${++sequence}`,
    analytics: false,
    fetch: (async (url: string, init: RequestInit) => {
      const headers: Record<string, string> = {};
      new Headers(init.headers).forEach((value, key) => {
        headers[key] = value;
      });
      requests.push({ url: String(url), headers });
      if (String(url).endsWith("/shopper/sessions")) {
        return jsonResponse(201, { data: { shopper_token: "shopper-token-abc" } });
      }
      return jsonResponse(201, { data: { id: "cart-1", lines: [] } });
    }) as unknown as typeof fetch,
  });

  await client.cart.create();
  await client.cart.get("cart-1");

  assert.equal(requests.length, 3);
  assert.match(requests[0]!.url, /\/shopper\/sessions$/);
  assert.equal(requests[1]!.headers["x-chaos-shopper-token"], "shopper-token-abc");
  assert.equal(requests[2]!.headers["x-chaos-shopper-token"], "shopper-token-abc");
  assert.equal(client.getShopperToken(), "shopper-token-abc");
});

test("reuses a shopper token persisted from a previous session", async () => {
  const storage = new MemoryStorage();
  const firstClient = createStorefrontClient({
    publishableKey: "public_test",
    storage,
    analytics: false,
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });
  firstClient.setShopperToken("existing-token");
  const requests: string[] = [];
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage,
    analytics: false,
    fetch: (async (url: string) => {
      requests.push(String(url));
      return jsonResponse(200, { data: { id: "cart-1", lines: [] } });
    }) as unknown as typeof fetch,
  });

  await client.cart.get("cart-1");

  assert.equal(requests.length, 1);
  assert.doesNotMatch(requests[0]!, /shopper\/sessions/);
});

test("explicit shopper sessions update the client token", async () => {
  const storage = new MemoryStorage();
  const client = createStorefrontClient({
    publishableKey: "public_test",
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
    publishableKey: "public_test",
    storage,
    analytics: false,
    retryInvalidShopperToken: true,
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

test("can fail on a stale shopper token without silently changing identity", async () => {
  const requests: string[] = [];
  const client = createStorefrontClient({
    publishableKey: "public_test",
    baseUrl: "https://shop.example.com/storefront/v1",
    storage: null,
    analytics: false,
    retryInvalidShopperToken: false,
    fetch: (async (url: string) => {
      requests.push(url);
      return jsonResponse(401, { error: { code: "unauthorized" } });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("stale-token");

  await assert.rejects(client.cart.get("cart-1"), (error: unknown) => {
    return error instanceof ChaosApiError && error.status === 401;
  });

  assert.equal(requests.length, 1);
  assert.equal(client.getShopperToken(), "stale-token");
});

test("can require an explicitly seeded shopper token", async () => {
  let requestCount = 0;
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    analytics: false,
    autoAcquireShopperToken: false,
    fetch: (async () => {
      requestCount += 1;
      return jsonResponse(200, { data: {} });
    }) as unknown as typeof fetch,
  });

  await assert.rejects(client.cart.get("cart-1"), (error: unknown) => {
    return error instanceof ChaosApiError && error.code === "shopper_token_required";
  });

  assert.equal(requestCount, 0);
});

test("creates a fresh cart when the stored cart has completed checkout", async () => {
  const requests: Array<{ url: string; token: string | null }> = [];
  const client = createStorefrontClient({
    publishableKey: "public_test",
    baseUrl: "https://shop.example.com/storefront/v1",
    storage: null,
    analytics: false,
    fetch: (async (url: string, init: RequestInit) => {
      const token = new Headers(init.headers).get("x-chaos-shopper-token");
      requests.push({ url, token });
      if (url.endsWith("/carts/completed-cart")) {
        return jsonResponse(200, { data: { id: "completed-cart", status: "completed", lines: [] } });
      }
      return jsonResponse(201, { data: { id: "fresh-cart", status: "active", lines: [] } });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("stable-shopper-token");

  const response = await client.cart.getOrCreate("completed-cart");

  assert.equal(response.data.id, "fresh-cart");
  assert.deepEqual(
    requests.map((request) => [request.url, request.token]),
    [
      ["https://shop.example.com/storefront/v1/carts/completed-cart", "stable-shopper-token"],
      ["https://shop.example.com/storefront/v1/carts", "stable-shopper-token"],
    ],
  );
  assert.equal(client.getShopperToken(), "stable-shopper-token");
});

test("shares one shopper-session request across concurrent explicit acquisitions", async () => {
  let sessionRequests = 0;
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    analytics: false,
    fetch: (async (url: string) => {
      if (url.endsWith("/shopper/sessions")) {
        sessionRequests += 1;
        await new Promise((resolve) => setTimeout(resolve, 0));
        return jsonResponse(201, { data: { shopper_token: "shared-token" } });
      }
      return jsonResponse(200, { data: {} });
    }) as unknown as typeof fetch,
  });

  const tokens = await Promise.all([
    client.acquireShopperToken(),
    client.acquireShopperToken(),
  ]);

  assert.deepEqual(tokens, ["shared-token", "shared-token"]);
  assert.equal(sessionRequests, 1);
});

test("serializes concurrent addLine calls for one cart", async () => {
  let quantity = 1;
  let version = 0;
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    analytics: false,
    randomUUID: () => "random-id",
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return jsonResponse(201, { data: { shopper_token: "shopper-token" } });
      }
      if (init.method === "GET") {
        await new Promise((resolve) => setTimeout(resolve, 0));
        return jsonResponse(200, {
          data: {
            id: "cart-1",
            version,
            lines: [{ product_variant_id: "variant-1", quantity }],
          },
        });
      }
      quantity = JSON.parse(String(init.body)).quantity;
      version += 1;
      return jsonResponse(200, {
        data: {
          id: "cart-1",
          version,
          lines: [{ product_variant_id: "variant-1", quantity }],
        },
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
    publishableKey: "public_test",
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
    publishableKey: "public_test",
    baseUrl: "https://shop.example.com/storefront/v1",
    storage: new MemoryStorage(),
    analytics: false,
    fetch: (async (url: string) => {
      captured.url = new URL(String(url));
      return jsonResponse(200, { data: [], meta: { page: { has_more: false } } });
    }) as unknown as typeof fetch,
  });

  await client.catalog.listProducts({ q: "shoes", limit: 10, collection: "sale" });

  assert.equal(captured.url?.pathname, "/storefront/v1/products");
  assert.equal(captured.url?.searchParams.get("q"), "shoes");
  assert.equal(captured.url?.searchParams.get("limit"), "10");
  assert.equal(captured.url?.searchParams.get("collection"), "sale");
});

test("payments create an embedded Checkout session in one request", async () => {
  const requests: Array<{ url: string; method: string; headers: Headers; body: string | undefined }> = [];
  let sequence = 0;
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    analytics: false,
    randomUUID: () => `id-${++sequence}`,
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({
        url,
        method: init.method ?? "GET",
        headers: new Headers(init.headers),
        body: typeof init.body === "string" ? init.body : undefined,
      });
      if (url.endsWith("/shopper/sessions")) {
        return jsonResponse(201, { data: { shopper_token: "shopper-token" } });
      }
      if (url.endsWith("/checkout")) {
        return jsonResponse(201, {
          data: {
            order_id: "order-1",
            client_action: {
              type: "mount_embedded_checkout",
              public_key: "pk_test_stripe",
              client_token: "cs_test_secret",
            },
          },
        });
      }
      return jsonResponse(404, { error: { code: "not_found", message: "not found" } });
    }) as unknown as typeof fetch,
  });

  const session = await client.payments.createEmbeddedCheckout("cart-1", {
    email: "shopper@example.com",
    payment_provider: "stripe",
    return_url: "https://shop.example.com/checkout/success",
  }, "order-idempotency-1");

  assert.equal(requests[1]?.headers.get("x-chaos-shopper-token"), "shopper-token");
  assert.equal(requests[1]?.headers.get("idempotency-key"), "order-idempotency-1");
  assert.deepEqual(JSON.parse(requests[1]?.body ?? "{}"), {
    email: "shopper@example.com",
    payment_provider: "stripe",
    return_url: "https://shop.example.com/checkout/success",
  });
  assert.deepEqual(session.data.client_action, {
    type: "mount_embedded_checkout",
    public_key: "pk_test_stripe",
    client_token: "cs_test_secret",
  });
});

test("payments create an embedded Checkout session without an email", async () => {
  const requests: Array<{ url: string; body: string | undefined }> = [];
  let sequence = 0;
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    analytics: false,
    randomUUID: () => `id-${++sequence}`,
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({ url, body: typeof init.body === "string" ? init.body : undefined });
      if (url.endsWith("/shopper/sessions")) {
        return jsonResponse(201, { data: { shopper_token: "shopper-token" } });
      }
      if (url.endsWith("/checkout")) {
        return jsonResponse(201, {
          data: {
            order_id: "order-1",
            client_action: {
              type: "mount_embedded_checkout",
              public_key: "pk_test_stripe",
              client_token: "cs_test_secret",
            },
          },
        });
      }
      return jsonResponse(404, { error: { code: "not_found", message: "not found" } });
    }) as unknown as typeof fetch,
  });

  await client.payments.createEmbeddedCheckout("cart-1", {
    payment_provider: "stripe",
    return_url: "https://shop.example.com/checkout/success",
  }, "order-idempotency-1");

  assert.deepEqual(JSON.parse(requests[1]?.body ?? "{}"), {
    payment_provider: "stripe",
    return_url: "https://shop.example.com/checkout/success",
  });
});

test("browser SDK observations record only after successful responses", async () => {
  const recorded: Array<[string, unknown]> = [];
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage: new MemoryStorage(),
    analytics: false,
    randomUUID: () => "random-id",
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
  };
  mutable.request = async (path) => {
    if (path === "/products") return { data: [{ id: "product-1" }], meta: { page: { has_more: false } } };
    if (path.startsWith("/products/")) return { data: { id: "product-1" } };
    if (path === "/carts/cart-1") return { data: { id: "cart-1", lines: [] } };
    return { data: { id: "cart-1", lines: [{ product_variant_id: "variant-1", quantity: 2 }] } };
  };

  await client.catalog.listProducts({ q: "shoes" });
  await client.catalog.getProduct("shoe");
  await client.cart.addLine("cart-1", "variant-1", 2);

  assert.deepEqual(recorded, [
    ["search", { query: "shoes", resultCount: 1 }],
    ["view_content", { productId: "product-1" }],
  ]);
});

test("projects purchases only after a confirmed order is paid", async () => {
  const client = createStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    analytics: false,
    randomUUID: () => "random-id",
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });
  const recorded: unknown[] = [];
  const mutable = client as unknown as {
    analytics: { purchase: (input: unknown) => void };
    request: () => Promise<unknown>;
  };
  mutable.analytics = { purchase: (input) => recorded.push(input) };
  const order = {
    id: "order-1",
    status: "confirmed",
    payment_status: "pending",
    total_amount_minor: 1_000,
    currency: "USD",
    lines: [{ product_variant_id: "variant-1", quantity: 1, unit_price_amount_minor: 1_000 }],
  };
  mutable.request = async () => ({ data: order });

  await client.orders.get("order-1");
  assert.equal(recorded.length, 0);

  order.payment_status = "paid";
  await client.orders.get("order-1");
  assert.equal(recorded.length, 1);
});
