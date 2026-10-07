import assert from "node:assert/strict";
import test from "node:test";

import { ChaosStorefrontClient } from "../client.js";
import { ChaosApiError } from "../errors.js";
import { defaultAdAttribution } from "../internal/attribution.js";
import { storefrontStorageKeys } from "../internal/browser-storage.js";
import type { CheckoutOrder, OrderLookup, OwnOrder } from "../types.js";

// The package is browser-only. Individual tests replace these minimal globals
// when they need a specific URL, cookie jar or DOM behavior.
Object.defineProperty(globalThis, "window", {
  value: { location: { href: "", origin: "https://shop.example.com" } },
  configurable: true,
});
Object.defineProperty(globalThis, "document", {
  value: { cookie: "", location: { search: "", protocol: "http:" } },
  configurable: true,
});

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

const TEST_SHOPPER_ID = "00000000-0000-4000-8000-000000000099";

function shopperSessionResponse(token: string): Response {
  return jsonResponse(201, {
    data: { shopper_id: TEST_SHOPPER_ID, shopper_token: token },
  });
}

function shopperToken(headers: Headers): string | null {
  return headers.get("x-chaos-shopper-token");
}

function checkoutOrder(overrides: Partial<OrderLookup> = {}): OrderLookup {
  return {
    id: "00000000-0000-4000-8000-000000000001",
    order_number: "W-12345678",
    currency: "USD",
    status: "pending",
    payment_status: "pending",
    fulfillment_status: "pending",
    subtotal_amount_minor: 2_000,
    discount_amount_minor: 0,
    tax_amount_minor: 0,
    shipping_amount_minor: 0,
    total_amount_minor: 2_000,
    refunded_amount_minor: 0,
    fulfillments: [],
    lines: [],
    created_at: "2026-10-07T00:00:00Z",
    updated_at: "2026-10-07T00:00:00Z",
    ...overrides,
  };
}

function checkoutCapabilityOrder(
  overrides: Partial<CheckoutOrder> = {},
): CheckoutOrder {
  const { fulfillments: _fulfillments, ...order } = checkoutOrder(overrides);
  return order;
}

function restoreGlobal(
  key: "document" | "window" | "localStorage" | "Stripe",
  descriptor: PropertyDescriptor | undefined,
): void {
  if (descriptor) Object.defineProperty(globalThis, key, descriptor);
  else Reflect.deleteProperty(globalThis, key);
}

test("checkout order lookup keeps the original shopper identity", async () => {
  const requests: Array<{ url: string; token: string | null }> = [];
  const order = {
    id: "00000000-0000-4000-8000-000000000001",
    order_number: "W-12345678",
    status: "pending",
    payment_status: "pending",
  } as OwnOrder;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    retryInvalidShopperToken: true,
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({ url: String(url), token: shopperToken(new Headers(init.headers)) });
      return jsonResponse(200, { data: order });
    }) as unknown as typeof fetch,
  });
  await assert.rejects(() => client.orders.getCheckoutOrder(order.id), (error: unknown) =>
    error instanceof ChaosApiError && error.code === "shopper_token_required");
  assert.equal(requests.length, 0);
  client.setShopperToken("shopper-original");
  const result = await client.orders.getCheckoutOrder(order.id);
  assert.equal(result.data, order);
  assert.deepEqual(requests, [{
    url: "https://shop.example.com/api/v1/orders/00000000-0000-4000-8000-000000000001/details",
    token: "shopper-original",
  }]);
});

test("waitForCheckoutOrder polls until paid and then projects Purchase", async () => {
  const states: OwnOrder[] = [
    {
      id: "00000000-0000-4000-8000-000000000001",
      order_number: "W-12345678",
      status: "pending",
      payment_status: "pending",
    } as OwnOrder,
    {
      id: "00000000-0000-4000-8000-000000000001",
      order_number: "W-12345678",
      status: "confirmed",
      payment_status: "paid",
    } as OwnOrder,
  ];
  let reads = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async () => {
      const order = states[Math.min(reads, states.length - 1)]!;
      reads += 1;
      return jsonResponse(200, { data: order });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("shopper-original");
  const purchases: string[] = [];
  client.recordConfirmedPurchase = (order) => purchases.push(order.id);

  const result = await client.orders.waitForCheckoutOrder(states[0]!.id, {
    intervalMs: 0,
    timeoutMs: 1_000,
  });

  assert.equal(result.data.payment_status, "paid");
  assert.equal(reads, 2);
  assert.deepEqual(purchases, [states[0]!.id]);
});

test("waitForCheckoutOrder can be cancelled before its first read", async () => {
  const controller = new AbortController();
  controller.abort();
  let reads = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async () => {
      reads += 1;
      return jsonResponse(200, { data: {} });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("shopper-original");

  await assert.rejects(
    () =>
      client.orders.waitForCheckoutOrder(
        "00000000-0000-4000-8000-000000000001",
        { signal: controller.signal },
      ),
    (error: unknown) =>
      error instanceof DOMException && error.name === "AbortError",
  );
  assert.equal(reads, 0);
});

test("waitForCheckoutOrder stops at its timeout", async () => {
  let reads = 0;
  const pendingOrder = {
    id: "00000000-0000-4000-8000-000000000001",
    order_number: "W-12345678",
    status: "pending",
    payment_status: "pending",
  } as OwnOrder;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async () => {
      reads += 1;
      return jsonResponse(200, { data: pendingOrder });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("shopper-original");

  await assert.rejects(
    () =>
      client.orders.waitForCheckoutOrder(pendingOrder.id, {
        intervalMs: 100,
        timeoutMs: 1,
      }),
    (error: unknown) =>
      error instanceof DOMException && error.name === "TimeoutError",
  );
  assert.equal(reads, 1);
});

test("mounted checkout confirms the Order before its in-place completion callback", async () => {
  const stripeDescriptor = Object.getOwnPropertyDescriptor(globalThis, "Stripe");
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  let stripeComplete: (() => void) | undefined;
  let mountedContainer: HTMLElement | undefined;
  Object.defineProperty(globalThis, "Stripe", {
    configurable: true,
    value: () => ({
      createEmbeddedCheckoutPage: async (options: { onComplete?: () => void }) => {
        stripeComplete = options.onComplete;
        return {
          mount: (container: HTMLElement) => {
            mountedContainer = container;
          },
          unmount: () => {},
          destroy: () => {},
        };
      },
    }),
  });
  const pending = checkoutOrder();
  const paid = checkoutOrder({ status: "confirmed", payment_status: "paid" });
  const checkoutUrl =
    `https://shop.example.com/checkout#order_id=${pending.id}` +
    "&checkout_token=checkout-token";
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      location: { href: checkoutUrl, origin: "https://shop.example.com" },
      history: { state: null, replaceState: () => {} },
    },
  });
  try {
    let reads = 0;
    const requestHeaders: Headers[] = [];
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      fetch: (async (_url: string, init: RequestInit) => {
        requestHeaders.push(new Headers(init.headers));
        const order = reads++ === 0 ? pending : paid;
        return jsonResponse(200, {
          data: {
            order,
            ...(order.payment_status === "pending"
              ? {
                  client_action: {
                    type: "stripe_checkout_embedded",
                    public_key: "pk_test_stripe",
                    client_token: "cs_test_secret",
                  },
                }
              : {}),
          },
        });
      }) as unknown as typeof fetch,
    });
    const container = {} as HTMLElement;
    let finish: (value: CheckoutOrder) => void = () => {};
    const completed = new Promise<CheckoutOrder>((resolve) => {
      finish = resolve;
    });

    await client.payments.mountEmbeddedCheckout(
      container,
      { onComplete: finish },
    );
    assert.equal(mountedContainer, container);
    assert.ok(stripeComplete);
    assert.equal(requestHeaders[0]?.get("x-chaos-checkout-token"), "checkout-token");
    assert.equal(shopperToken(requestHeaders[0]!), null);

    stripeComplete();
    assert.equal(await completed, paid);
    assert.equal(requestHeaders[1]?.get("x-chaos-checkout-token"), "checkout-token");
  } finally {
    restoreGlobal("Stripe", stripeDescriptor);
    restoreGlobal("window", windowDescriptor);
  }
});

test("a terminal shared checkout completes from its URL without mounting Stripe", async () => {
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  const order = checkoutOrder({ status: "confirmed", payment_status: "paid" });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      location: {
        href:
          `https://shop.example.com/checkout#order_id=${order.id}` +
          "&checkout_token=shared-token",
        origin: "https://shop.example.com",
      },
      history: { state: null, replaceState: () => {} },
    },
  });
  try {
    let headers: Headers | undefined;
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      fetch: (async (_url: string, init: RequestInit) => {
        headers = new Headers(init.headers);
        return jsonResponse(200, { data: { order } });
      }) as unknown as typeof fetch,
    });
    const purchases: string[] = [];
    client.recordConfirmedPurchase = (confirmed) => purchases.push(confirmed.id);
    let completed: CheckoutOrder | undefined;

    const mounted = await client.payments.mountEmbeddedCheckout(
      {} as HTMLElement,
      { onComplete: (confirmed) => { completed = confirmed; } },
    );

    assert.equal(mounted, null);
    assert.equal(completed, order);
    assert.deepEqual(purchases, [order.id]);
    assert.equal(headers?.get("x-chaos-checkout-token"), "shared-token");
    assert.equal(shopperToken(headers!), null);
  } finally {
    restoreGlobal("window", windowDescriptor);
  }
});

test("a failed shared checkout completes without projecting Purchase", async () => {
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  const order = checkoutCapabilityOrder({
    status: "cancelled",
    payment_status: "failed",
  });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      location: {
        href:
          `https://shop.example.com/checkout#order_id=${order.id}` +
          "&checkout_token=failed-token",
        origin: "https://shop.example.com",
      },
      history: { state: null, replaceState: () => {} },
    },
  });
  try {
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      fetch: (async () =>
        jsonResponse(200, { data: { order } })) as unknown as typeof fetch,
    });
    const purchases: string[] = [];
    client.recordConfirmedPurchase = (confirmed) => purchases.push(confirmed.id);
    let completed: CheckoutOrder | undefined;

    const mounted = await client.payments.mountEmbeddedCheckout(
      {} as HTMLElement,
      { onComplete: (terminal) => { completed = terminal; } },
    );

    assert.equal(mounted, null);
    assert.equal(completed, order);
    assert.deepEqual(purchases, []);
  } finally {
    restoreGlobal("window", windowDescriptor);
  }
});

test("checkout recovery records Purchase from the capability order", async () => {
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  const order = checkoutCapabilityOrder({
    status: "confirmed",
    payment_status: "paid",
  });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      location: {
        href:
          `https://shop.example.com/checkout#order_id=${order.id}` +
          "&checkout_token=owner-token",
        origin: "https://shop.example.com",
      },
      history: { state: null, replaceState: () => {} },
    },
  });
  try {
    const requests: Array<{ url: string; headers: Headers }> = [];
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      fetch: (async (url: string, init: RequestInit) => {
        requests.push({ url, headers: new Headers(init.headers) });
        return jsonResponse(200, { data: { order } });
      }) as unknown as typeof fetch,
    });
    client.setShopperToken("shopper-original");
    const purchases: Array<CheckoutOrder | OwnOrder> = [];
    client.recordCheckoutPurchase = async (confirmed) => {
      purchases.push(confirmed);
    };
    let completed: CheckoutOrder | undefined;

    const mounted = await client.payments.mountEmbeddedCheckout(
      {} as HTMLElement,
      { onComplete: (terminal) => { completed = terminal; } },
    );

    assert.equal(mounted, null);
    assert.equal(completed, order);
    assert.deepEqual(purchases, [order]);
    assert.equal(requests.length, 1);
    assert.equal(
      requests[0]?.headers.get("x-chaos-checkout-token"),
      "owner-token",
    );
    assert.equal(shopperToken(requests[0]!.headers), null);
  } finally {
    restoreGlobal("window", windowDescriptor);
  }
});

test("a pending checkout without a client action is rejected", async () => {
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  const order = checkoutCapabilityOrder();
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      location: {
        href:
          `https://shop.example.com/checkout#order_id=${order.id}` +
          "&checkout_token=broken-token",
        origin: "https://shop.example.com",
      },
      history: { state: null, replaceState: () => {} },
    },
  });
  try {
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      fetch: (async () =>
        jsonResponse(200, { data: { order } })) as unknown as typeof fetch,
    });

    await assert.rejects(
      () => client.payments.mountEmbeddedCheckout({} as HTMLElement),
      (error: unknown) =>
        error instanceof ChaosApiError &&
        error.code === "invalid_checkout_response",
    );
  } finally {
    restoreGlobal("window", windowDescriptor);
  }
});

test("guest order search uses GET with number and email, without a shopper token", async () => {
  const requests: Array<{ url: string; method: string | undefined; token: string | null }> = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({
        url: String(url),
        method: init.method,
        token: shopperToken(new Headers(init.headers)),
      });
      return jsonResponse(200, { data: { order_number: "W-12345678" } });
    }) as unknown as typeof fetch,
  });
  await client.orders.lookupOrder({ orderNumber: "W-12345678", email: "user@example.com" });
  assert.deepEqual(requests, [{
    url: "https://shop.example.com/api/v1/orders/search?order_number=W-12345678&email=user%40example.com",
    method: "GET",
    token: null,
  }]);
});

test("every confirmed checkout read attempts browser Purchase", async () => {
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test", storage: null,
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });
  const calls: string[] = [];
  client.recordConfirmedPurchase = (order) => { calls.push(order.id); };
  const order = {
    id: "00000000-0000-4000-8000-000000000001",
    order_number: "W-12345678", status: "confirmed", payment_status: "paid",
  } as OwnOrder;
  await client.recordCheckoutPurchase({ ...order, payment_status: "pending" });
  await client.recordCheckoutPurchase(order);
  await client.recordCheckoutPurchase(order);
  assert.deepEqual(calls, [order.id, order.id]);
});

test("defers shopper session creation until a browser request needs it", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    value: {},
    configurable: true,
  });
  const requests: string[] = [];
  try {
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      fetch: (async (url: string) => {
        requests.push(url);
        return shopperSessionResponse("browser-shopper-token");
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

test("rejects client construction outside a browser", () => {
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  const documentDescriptor = Object.getOwnPropertyDescriptor(globalThis, "document");
  Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "document");
  try {
    assert.throws(
      () =>
        new ChaosStorefrontClient({
          publishableKey: "public_test",
          fetch: (async () => jsonResponse(200, {})) as unknown as typeof fetch,
        }),
      /browser-only/,
    );
  } finally {
    restoreGlobal("window", windowDescriptor);
    restoreGlobal("document", documentDescriptor);
  }
});

test("acquires a shopper session on the first shopper-scoped request and reuses it", async () => {
  const requests: Array<{ url: string; headers: Record<string, string> }> = [];
  let sequence = 0;
  const storage = new MemoryStorage();
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage,
    randomUUID: () => `id-${++sequence}`,
    fetch: (async (url: string, init: RequestInit) => {
      const headers: Record<string, string> = {};
      new Headers(init.headers).forEach((value, key) => {
        headers[key] = value;
      });
      requests.push({ url: String(url), headers });
      if (String(url).endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token-abc");
      }
      return jsonResponse(201, { data: { id: "cart-1", lines: [] } });
    }) as unknown as typeof fetch,
  });

  await client.cart.create();
  await client.cart.get("cart-1");

  assert.equal(requests.length, 3);
  assert.match(requests[0]!.url, /\/shopper\/sessions$/);
  assert.equal(requests[0]!.headers["x-chaos-publishable-key"], "public_test");
  assert.equal(requests[0]!.headers["x-chaos-shopper-token"], undefined);
  assert.equal(requests[0]!.headers.authorization, undefined);
  assert.equal(
    requests[1]!.headers["x-chaos-shopper-token"],
    "shopper-token-abc",
  );
  assert.equal(
    requests[2]!.headers["x-chaos-shopper-token"],
    "shopper-token-abc",
  );
  assert.equal(requests[1]!.headers["x-chaos-publishable-key"], "public_test");
  assert.equal(requests[2]!.headers["x-chaos-publishable-key"], "public_test");
  assert.equal(requests[1]!.headers.authorization, undefined);
  assert.equal(requests[2]!.headers.authorization, undefined);
  assert.equal(client.getShopperToken(), "shopper-token-abc");
});

test("reuses a shopper token persisted from a previous session", async () => {
  const storage = new MemoryStorage();
  const firstClient = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage,
    fetch: (async () =>
      jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });
  firstClient.setShopperToken("existing-token");
  const requests: string[] = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage,
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
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage,
    fetch: (async () =>
      shopperSessionResponse("manual-token")) as unknown as typeof fetch,
  });

  await client.shopperSession.create();

  assert.equal(client.getShopperToken(), "manual-token");
});

test("does not persist a malformed shopper-session response", async () => {
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async () =>
      jsonResponse(201, {
        data: {},
      })) as unknown as typeof fetch,
  });

  await assert.rejects(client.shopperSession.create(), (error: unknown) => {
    return (
      error instanceof ChaosApiError &&
      error.code === "invalid_shopper_session_response"
    );
  });
  assert.equal(client.getShopperToken(), null);
});

test("refreshes a stale shopper token once and retries the request", async () => {
  const storage = new MemoryStorage();
  const requests: Array<{ url: string; token: string | undefined }> = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage,
    retryInvalidShopperToken: true,
    fetch: (async (url: string, init: RequestInit) => {
      const headers = new Headers(init.headers);
      requests.push({
        url,
        token: shopperToken(headers) ?? undefined,
      });
      if (url.endsWith("/carts/cart-1")) {
        if (requests.at(-1)?.token === "stale-token")
          return jsonResponse(401, { error: { code: "shopper_token_invalid" } });
        return jsonResponse(200, { data: { id: "cart-1", lines: [] } });
      }
      return shopperSessionResponse("fresh-token");
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
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    baseUrl: "https://shop.example.com/api/v1",
    storage: null,
    retryInvalidShopperToken: false,
    fetch: (async (url: string) => {
      requests.push(url);
      return jsonResponse(401, { error: { code: "shopper_token_invalid" } });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("stale-token");

  await assert.rejects(client.cart.get("cart-1"), (error: unknown) => {
    return error instanceof ChaosApiError && error.status === 401;
  });

  assert.equal(requests.length, 1);
  assert.equal(client.getShopperToken(), "stale-token");
});

test("does not rotate shopper identity for a publishable-key failure", async () => {
  let requestCount = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "invalid_key",
    storage: null,
    retryInvalidShopperToken: true,
    fetch: (async () => {
      requestCount += 1;
      return jsonResponse(401, {
        error: {
          code: "publishable_key_invalid",
          message: "the publishable key is invalid",
        },
      });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("stable-shopper-token");

  await assert.rejects(client.cart.get("cart-1"), (error: unknown) => {
    return (
      error instanceof ChaosApiError &&
      error.code === "publishable_key_invalid"
    );
  });

  assert.equal(requestCount, 1);
  assert.equal(client.getShopperToken(), "stable-shopper-token");
});

test("can require an explicitly seeded shopper token", async () => {
  let requestCount = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    autoAcquireShopperToken: false,
    fetch: (async () => {
      requestCount += 1;
      return jsonResponse(200, { data: {} });
    }) as unknown as typeof fetch,
  });

  await assert.rejects(client.cart.get("cart-1"), (error: unknown) => {
    return (
      error instanceof ChaosApiError && error.code === "shopper_token_required"
    );
  });

  assert.equal(requestCount, 0);
});

test("creates a fresh cart when the supplied cart is locked", async () => {
  const requests: Array<{ url: string; token: string | null }> = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    baseUrl: "https://shop.example.com/api/v1",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      const token = shopperToken(new Headers(init.headers));
      requests.push({ url, token });
      if (url.endsWith("/carts/locked-cart")) {
        return jsonResponse(200, {
          data: { id: "locked-cart", status: "locked", lines: [] },
        });
      }
      return jsonResponse(201, {
        data: { id: "fresh-cart", status: "active", lines: [] },
      });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("stable-shopper-token");

  const response = await client.cart.getOrCreate("locked-cart");

  assert.equal(response.data.id, "fresh-cart");
  assert.deepEqual(
    requests.map((request) => [request.url, request.token]),
    [
      [
        "https://shop.example.com/api/v1/carts/locked-cart",
        "stable-shopper-token",
      ],
      ["https://shop.example.com/api/v1/carts", "stable-shopper-token"],
    ],
  );
  assert.equal(client.getShopperToken(), "stable-shopper-token");
});

test("shares one shopper-session request across concurrent explicit acquisitions", async () => {
  let sessionRequests = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string) => {
      if (url.endsWith("/shopper/sessions")) {
        sessionRequests += 1;
        await new Promise((resolve) => setTimeout(resolve, 0));
        return shopperSessionResponse("shared-token");
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
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    randomUUID: () => "random-id",
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (init.method === "GET") {
        await new Promise((resolve) => setTimeout(resolve, 0));
        return jsonResponse(200, {
          data: {
            id: "cart-1",
            lines: [{ product_variant_id: "variant-1", quantity }],
          },
        });
      }
      quantity = JSON.parse(String(init.body)).quantity;
      return jsonResponse(200, {
        data: {
          id: "cart-1",
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
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: new MemoryStorage(),
    fetch: (async () =>
      jsonResponse(422, {
        error: {
          code: "validation_failed",
          message: "quantity must be at least 1",
          details: [{ field: "quantity", reason: "must be >= 1" }],
        },
      })) as unknown as typeof fetch,
  });

  await assert.rejects(client.catalog.listProducts(), (error: unknown) => {
    if (!(error instanceof ChaosApiError)) return false;
    assert.equal(error.status, 422);
    assert.equal(error.code, "validation_failed");
    assert.deepEqual(error.details, [
      { field: "quantity", reason: "must be >= 1" },
    ]);
    return true;
  });
});

test("catalog.listProducts forwards query parameters", async () => {
  const captured: { url: URL | null } = { url: null };
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    baseUrl: "https://shop.example.com/api/v1",
    storage: new MemoryStorage(),
    fetch: (async (url: string) => {
      captured.url = new URL(String(url));
      return jsonResponse(200, {
        data: [],
        meta: { page: { has_more: false } },
      });
    }) as unknown as typeof fetch,
  });

  await client.catalog.listProducts({
    q: "shoes",
    limit: 10,
    collection: "sale",
  });

  assert.equal(captured.url?.pathname, "/api/v1/products");
  assert.equal(captured.url?.searchParams.get("q"), "shoes");
  assert.equal(captured.url?.searchParams.get("limit"), "10");
  assert.equal(captured.url?.searchParams.get("collection"), "sale");
});

test("catalog search records only the first page of a non-empty query", async () => {
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async () =>
      jsonResponse(200, {
        data: [],
        meta: { page: { has_more: false } },
      })) as unknown as typeof fetch,
  });
  const searches: string[] = [];
  client.recordSearch = ({ query }) => searches.push(query);

  await client.catalog.listProducts({ q: "  shoes  " });
  await client.catalog.listProducts({ q: "shoes", cursor: "next-page" });
  await client.catalog.listProducts({ q: "   " });

  assert.deepEqual(searches, ["shoes"]);
});

test("catalog.getProduct records a product view", async () => {
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async () =>
      jsonResponse(200, {
        data: {
          id: "product-1",
          handle: "running-shoe",
          title: "Running shoe",
          variants: [],
        },
      })) as unknown as typeof fetch,
  });
  let views = 0;
  client.recordProductView = () => {
    views += 1;
  };

  await client.catalog.getProduct("running-shoe");

  assert.equal(views, 1);
});

test("catalog.getProduct records the Product even when every Variant is sold out", async () => {
  const product = {
    id: "product-1",
    handle: "running-shoe",
    title: "Running shoe",
    description: "",
    media: [],
    collections: [],
    options: [
      {
        id: "color",
        name: "Color",
        position: 0,
        values: [
          { id: "black", value: "Black", position: 0 },
          { id: "white", value: "White", position: 1 },
        ],
      },
    ],
    variants: [
      {
        id: "variant-sold-out",
        title: "Black",
        track_inventory: true,
        available_quantity: 0,
        price: { amount_minor: 1_000, currency: "USD" },
        selected_options: [
          { option_id: "color", option_value_id: "black" },
        ],
      },
    ],
  };
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async () => jsonResponse(200, { data: product })) as unknown as typeof fetch,
  });
  const views: string[] = [];
  client.recordProductView = (openedProduct) => views.push(openedProduct.id);

  const response = await client.catalog.getProduct("running-shoe");
  assert.equal(response.data, product);
  assert.deepEqual(views, ["product-1"]);
});

test("payments create an embedded Checkout session with SDK-owned request details", async () => {
  const requests: Array<{
    url: string;
    method: string;
    headers: Headers;
    body: string | undefined;
  }> = [];
  let sequence = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    randomUUID: () => `id-${++sequence}`,
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({
        url,
        method: init.method ?? "GET",
        headers: new Headers(init.headers),
        body: typeof init.body === "string" ? init.body : undefined,
      });
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1")) {
        return jsonResponse(200, {
          data: {
            id: "cart-1",
            currency: "USD",
            subtotal_amount_minor: 2_000,
            lines: [],
          },
        });
      }
      if (url.endsWith("/checkout")) {
        return jsonResponse(201, {
          data: {
            order_id: "00000000-0000-4000-8000-000000000001",
            order_number: "W-20260830-00000001",
            checkout_token: "checkout-token",
            client_action: {
              type: "stripe_checkout_embedded",
              public_key: "pk_test_stripe",
              client_token: "cs_test_secret",
            },
          },
        });
      }
      return jsonResponse(404, {
        error: { code: "cart_not_found", message: "not found" },
      });
    }) as unknown as typeof fetch,
  });
  const recordedCheckouts: unknown[] = [];
  client.recordCheckoutCreation = (checkout) => recordedCheckouts.push(checkout);

  const session = await client.payments.createEmbeddedCheckout("cart-1");

  assert.equal(
    shopperToken(requests[2]!.headers),
    "shopper-token",
  );
  assert.equal(requests[2]?.headers.get("idempotency-key"), "id-1");
  assert.deepEqual(JSON.parse(requests[2]?.body ?? "{}"), {
    payment_provider: "stripe",
  });
  assert.deepEqual(session.data.client_action, {
    type: "stripe_checkout_embedded",
    public_key: "pk_test_stripe",
    client_token: "cs_test_secret",
  });
  assert.equal(session.data.checkout_token, "checkout-token");
  assert.match(
    session.data.checkout_url,
    /#order_id=00000000-0000-4000-8000-000000000001&checkout_token=checkout-token$/,
  );
  assert.deepEqual(recordedCheckouts, [
    {
      checkout: session.data,
      source_cart: {
        id: "cart-1",
        currency: "USD",
        subtotal_amount_minor: 2_000,
        lines: [],
      },
    },
  ]);
});

test("checkout attaches explicit attribution and excludes it from the idempotency key", async () => {
  const requests: Array<{ url: string; headers: Headers; body: string | undefined }> = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({
        url,
        headers: new Headers(init.headers),
        body: typeof init.body === "string" ? init.body : undefined,
      });
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1")) {
        return jsonResponse(200, {
          data: { id: "cart-1", currency: "USD", subtotal_amount_minor: 2_000, lines: [] },
        });
      }
      if (url.endsWith("/checkout")) {
        return jsonResponse(201, {
          data: {
            order_id: "00000000-0000-4000-8000-000000000001",
            order_number: "W-20260830-00000001",
            checkout_token: "checkout-token",
            client_action: {
              type: "stripe_checkout_embedded",
              public_key: "pk_test_stripe",
              client_token: "cs_test_secret",
            },
          },
        });
      }
      return jsonResponse(404, { error: { code: "cart_not_found", message: "not found" } });
    }) as unknown as typeof fetch,
  });

  await client.payments.createEmbeddedCheckout("cart-1", {
    attribution: { meta: { fbc: "fb.1.1699999999999.click" } },
  });
  const firstCheckout = requests.find((request) => request.url.endsWith("/checkout"));
  assert.deepEqual(JSON.parse(firstCheckout?.body ?? "{}"), {
    payment_provider: "stripe",
    attribution: { meta: { fbc: "fb.1.1699999999999.click" } },
  });

  requests.length = 0;
  await client.payments.createEmbeddedCheckout("cart-1", {
    attribution: { meta: { fbc: "fb.1.1699999999999.a-different-click" } },
  });
  const secondCheckout = requests.find((request) => request.url.endsWith("/checkout"));
  assert.equal(
    secondCheckout?.headers.get("idempotency-key"),
    firstCheckout?.headers.get("idempotency-key"),
    "attribution must not change the idempotency key, or a retry with fresh attribution would mint a second checkout",
  );
});

test("checkout defaults source_url to the current page in a browser", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    value: {
      location: {
        href: "https://shop.example.com/checkout",
        origin: "https://shop.example.com",
      },
    },
    configurable: true,
  });
  try {
    let checkoutBody: string | undefined;
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      fetch: (async (url: string, init: RequestInit) => {
        if (url.endsWith("/shopper/sessions")) {
          return shopperSessionResponse("shopper-token");
        }
        if (url.endsWith("/carts/cart-1")) {
          return jsonResponse(200, {
            data: { id: "cart-1", currency: "USD", subtotal_amount_minor: 2_000, lines: [] },
          });
        }
        if (url.endsWith("/checkout")) {
          checkoutBody = typeof init.body === "string" ? init.body : undefined;
          return jsonResponse(201, {
            data: {
              order_id: "00000000-0000-4000-8000-000000000001",
              order_number: "W-20260830-00000001",
              checkout_token: "checkout-token",
              client_action: {
                type: "stripe_checkout_embedded",
                public_key: "pk_test_stripe",
                client_token: "cs_test_secret",
              },
            },
          });
        }
        return jsonResponse(404, { error: { code: "cart_not_found", message: "not found" } });
      }) as unknown as typeof fetch,
    });

    await client.payments.createEmbeddedCheckout("cart-1");

    assert.deepEqual(JSON.parse(checkoutBody ?? "{}").attribution, {
      source_url: "https://shop.example.com/checkout",
    });
  } finally {
    if (descriptor) {
      Object.defineProperty(globalThis, "window", descriptor);
    } else {
      Reflect.deleteProperty(globalThis, "window");
    }
  }
});

test("checkout captures Meta click attribution without browser event providers", async () => {
  const priorDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const priorWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const documentRef = {
    cookie: "",
    location: {
      protocol: "https:",
      search: "?fbclid=checkout-click",
    },
  };
  Object.defineProperty(globalThis, "document", {
    value: documentRef,
    configurable: true,
  });
  Object.defineProperty(globalThis, "window", {
    value: {
      location: {
        href: "https://shop.example.com/products/shoe",
        origin: "https://shop.example.com",
      },
    },
    configurable: true,
  });
  try {
    let checkoutBody: string | undefined;
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      now: () => 1_234_567_890_123,
      fetch: (async (url: string, init: RequestInit) => {
        if (url.endsWith("/shopper/sessions")) {
          return shopperSessionResponse("shopper-token");
        }
        if (url.endsWith("/carts/cart-1")) {
          return jsonResponse(200, {
            data: {
              id: "cart-1",
              currency: "USD",
              subtotal_amount_minor: 2_000,
              lines: [],
            },
          });
        }
        if (url.endsWith("/checkout")) {
          checkoutBody = typeof init.body === "string" ? init.body : undefined;
          return jsonResponse(201, {
            data: {
              order_id: "00000000-0000-4000-8000-000000000001",
              order_number: "W-20260830-00000001",
              checkout_token: "checkout-token",
              client_action: {
                type: "stripe_checkout_embedded",
                public_key: "pk_test_stripe",
                client_token: "cs_test_secret",
              },
            },
          });
        }
        return jsonResponse(404, {
          error: { code: "cart_not_found", message: "not found" },
        });
      }) as unknown as typeof fetch,
    });

    await client.payments.createEmbeddedCheckout("cart-1");

    assert.deepEqual(JSON.parse(checkoutBody ?? "{}").attribution, {
      source_url: "https://shop.example.com/products/shoe",
      meta: { fbc: "fb.1.1234567890123.checkout-click" },
    });
  } finally {
    restoreGlobal("document", priorDocument);
    restoreGlobal("window", priorWindow);
  }
});

test("browser events inherit the client's attribution clock", () => {
  const documentRef = {
    cookie: "",
    location: {
      protocol: "https:",
      search: "?fbclid=event-click",
    },
    createElement: () => ({ set async(_value: boolean) {}, src: "" }),
    head: { appendChild: () => undefined },
  };

  new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    now: () => 1_234_567_890_123,
    randomUUID: () => "00000000-0000-4000-8000-000000000001",
    events: {
      document: documentRef as unknown as Document,
      window: {} as Window & typeof globalThis,
    },
    fetch: (async () => jsonResponse(200, {})) as unknown as typeof fetch,
  });

  assert.match(
    documentRef.cookie,
    /^_fbc=fb\.1\.1234567890123\.event-click;/,
  );
});

test("checkout attribution tolerates compact and malformed cookies", () => {
  const priorDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    value: { cookie: "_fbp=malformed%E0%A4%A;other=value" },
    configurable: true,
  });
  try {
    assert.deepEqual(defaultAdAttribution().meta, {
      fbp: "malformed%E0%A4%A",
    });
  } finally {
    restoreGlobal("document", priorDocument);
  }
});

test("checkout leaves UTM attribution to GA4 unless explicitly supplied", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    value: {
      location: {
        href: "https://shop.example.com/checkout?utm_source=newsletter&utm_medium=email&utm_campaign=fall",
        origin: "https://shop.example.com",
      },
    },
    configurable: true,
  });
  try {
    let checkoutBody: string | undefined;
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      storage: null,
      fetch: (async (url: string, init: RequestInit) => {
        if (url.includes("/shopper/sessions")) {
          return shopperSessionResponse("shopper-token");
        }
        if (url.endsWith("/carts/cart-1")) {
          return jsonResponse(200, {
            data: { id: "cart-1", currency: "USD", subtotal_amount_minor: 2_000, lines: [] },
          });
        }
        if (url.endsWith("/checkout")) {
          checkoutBody = typeof init.body === "string" ? init.body : undefined;
          return jsonResponse(201, {
            data: {
              order_id: "00000000-0000-4000-8000-000000000001",
              order_number: "W-20260830-00000001",
              checkout_token: "checkout-token",
              client_action: {
                type: "stripe_checkout_embedded",
                public_key: "pk_test_stripe",
                client_token: "cs_test_secret",
              },
            },
          });
        }
        return jsonResponse(404, { error: { code: "cart_not_found", message: "not found" } });
      }) as unknown as typeof fetch,
    });

    await client.payments.createEmbeddedCheckout("cart-1");

    assert.deepEqual(JSON.parse(checkoutBody ?? "{}").attribution, {
      source_url:
        "https://shop.example.com/checkout?utm_source=newsletter&utm_medium=email&utm_campaign=fall",
    });
  } finally {
    if (descriptor) {
      Object.defineProperty(globalThis, "window", descriptor);
    } else {
      Reflect.deleteProperty(globalThis, "window");
    }
  }
});

test("shopper session creation sends attribution in the JSON body", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    value: {
      location: {
        href: "https://shop.example.com/?utm_source=newsletter&utm_campaign=fall&other=x",
        origin: "https://shop.example.com",
      },
    },
    configurable: true,
  });
  const requests: Array<{ url: string; body: string | undefined }> = [];
  try {
    const client = new ChaosStorefrontClient({
      publishableKey: "public_test",
      baseUrl: "https://shop.example.com/api/v1",
      storage: null,
      fetch: (async (url: string, init: RequestInit) => {
        requests.push({
          url: String(url),
          body: typeof init.body === "string" ? init.body : undefined,
        });
        if (String(url).includes("/shopper/sessions")) {
          return shopperSessionResponse("shopper-token");
        }
        return jsonResponse(201, { data: { id: "cart-1", lines: [] } });
      }) as unknown as typeof fetch,
    });

    await client.cart.create();

    const session = requests.find((request) =>
      request.url.includes("/shopper/sessions"),
    );
    assert.ok(session, "a shopper session was created");
    assert.equal(new URL(session.url).search, "");
    assert.deepEqual(JSON.parse(session.body ?? "{}"), {
      attribution: {
        utm: { source: "newsletter", campaign: "fall" },
      },
    });
  } finally {
    if (descriptor) {
      Object.defineProperty(globalThis, "window", descriptor);
    } else {
      Reflect.deleteProperty(globalThis, "window");
    }
  }
});

test("shopper session creation forwards the first-touch utm_* even after the URL drops them", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  const storage = new MemoryStorage();
  const setHref = (href: string) =>
    Object.defineProperty(globalThis, "window", {
      value: { location: { href, origin: new URL(href).origin } },
      configurable: true,
    });
  try {
    // Landing page carries the campaign tags.
    setHref("https://shop.example.com/?utm_source=meta&utm_campaign=spring");
    const first = new ChaosStorefrontClient({
      publishableKey: "public_test",
      baseUrl: "https://shop.example.com/api/v1",
      storage,
      fetch: (async () =>
        shopperSessionResponse("t")) as unknown as typeof fetch,
    });
    await first.shopperSession.create();

    // A later MPA navigation: no utm_* on the URL, and a stale token so a new
    // session is minted. It must still carry the persisted first-touch tags.
    setHref("https://shop.example.com/products/x");
    const requests: Array<{ url: string; body: string | undefined }> = [];
    const later = new ChaosStorefrontClient({
      publishableKey: "public_test",
      baseUrl: "https://shop.example.com/api/v1",
      storage,
      fetch: (async (url: string, init: RequestInit) => {
        requests.push({
          url: String(url),
          body: typeof init.body === "string" ? init.body : undefined,
        });
        return shopperSessionResponse("t2");
      }) as unknown as typeof fetch,
    });
    later.setShopperToken(null);
    await later.shopperSession.create();

    assert.equal(new URL(requests[0]!.url).search, "");
    assert.deepEqual(JSON.parse(requests[0]!.body ?? "{}"), {
      attribution: { utm: { source: "meta", campaign: "spring" } },
    });
  } finally {
    if (descriptor) {
      Object.defineProperty(globalThis, "window", descriptor);
    } else {
      Reflect.deleteProperty(globalThis, "window");
    }
  }
});

test("browser storage keys use one versioned store scope", () => {
  const first = storefrontStorageKeys(
    "https://shop.example.com/api/v1",
    "public_first",
  );
  const second = storefrontStorageKeys(
    "https://shop.example.com/api/v1",
    "public_second",
  );

  assert.match(
    first.shopperToken,
    /^chaos\.storefront\.v1\.[^.]+\.shopper\.token$/,
  );
  assert.equal(
    first.shopperToken.replace(/\.shopper\.token$/, ""),
    first.attributionUtmFirst.replace(/\.attribution\.utm\.first$/, ""),
  );
  assert.notEqual(first.shopperToken, second.shopperToken);
  assert.notEqual(first.attributionUtmFirst, second.attributionUtmFirst);
});

test("UTM attribution is isolated by API and publishable key", () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  const storage = new MemoryStorage();
  const setHref = (href: string) =>
    Object.defineProperty(globalThis, "window", {
      value: { location: { href, origin: new URL(href).origin } },
      configurable: true,
    });
  const client = (publishableKey: string) =>
    new ChaosStorefrontClient({
      publishableKey,
      baseUrl: "https://shop.example.com/api/v1",
      storage,
      fetch: (async () =>
        jsonResponse(200, { data: {} })) as unknown as typeof fetch,
    });

  try {
    setHref("https://shop.example.com/?utm_source=first");
    client("public_first");
    setHref("https://shop.example.com/?utm_source=second");
    client("public_second");
    setHref("https://shop.example.com/products/shoe");

    assert.deepEqual(client("public_first").firstTouchUtm(), {
      source: "first",
    });
    assert.deepEqual(client("public_second").firstTouchUtm(), {
      source: "second",
    });
    const firstKeys = storefrontStorageKeys(
      "https://shop.example.com/api/v1",
      "public_first",
    );
    assert.deepEqual(
      JSON.parse(storage.getItem(firstKeys.attributionUtmFirst) ?? "null"),
      { source: "first" },
    );
  } finally {
    restoreGlobal("window", descriptor);
  }
});

test("cart line mutations report the resulting quantity delta to analytics", async () => {
  const mutations: unknown[] = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1") ) {
        return jsonResponse(200, {
          data: {
            id: "cart-1",
            currency: "USD",
            subtotal_amount_minor: 2_000,
            lines: [{ product_id: "p-1", product_variant_id: "v-1", quantity: 1, unit_price_amount_minor: 500 }],
          },
        });
      }
      // The PUT/DELETE line mutation itself.
      return jsonResponse(200, {
        data: {
          id: "cart-1",
          currency: "USD",
          subtotal_amount_minor: 1_500,
          lines: [{ product_id: "p-1", product_variant_id: "v-1", quantity: 3, unit_price_amount_minor: 500 }],
        },
      });
    }) as unknown as typeof fetch,
  });
  client.recordCartMutation = (mutation) => mutations.push(mutation);

  await client.cart.addLine("cart-1", "v-1", 2);

  assert.deepEqual(mutations, [
    {
      cart: {
        id: "cart-1",
        currency: "USD",
        subtotal_amount_minor: 1_500,
        lines: [{ product_id: "p-1", product_variant_id: "v-1", quantity: 3, unit_price_amount_minor: 500 }],
      },
      product_variant_id: "v-1",
      previous_quantity: 1,
      new_quantity: 3,
      removed: false,
    },
  ]);
});

test("a quantity-raising line mutation sends only cart data to the server", async () => {
  const mutations: unknown[] = [];
  const bodies: Array<{ method: string | undefined; body: unknown }> = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1")) {
        return jsonResponse(200, {
          data: {
            id: "cart-1",
            currency: "USD",
            subtotal_amount_minor: 0,
            lines: [],
          },
        });
      }
      bodies.push({
        method: init.method,
        body: typeof init.body === "string" ? JSON.parse(init.body) : undefined,
      });
      return jsonResponse(200, {
        data: {
          id: "cart-1",
          currency: "USD",
          subtotal_amount_minor: 500,
          lines: [{ product_id: "p-1", product_variant_id: "v-1", quantity: 1, unit_price_amount_minor: 500 }],
        },
      });
    }) as unknown as typeof fetch,
  });
  client.recordCartMutation = (mutation) => mutations.push(mutation);

  await client.cart.addLine("cart-1", "v-1", 1);

  assert.equal(bodies.length, 1);
  assert.equal(bodies[0]!.method, "PUT");
  assert.deepEqual(bodies[0]!.body, { quantity: 1 });
  assert.equal(mutations.length, 1);
});

test("checkout creation keeps the source Cart snapshot when rotating the Cart", async () => {
  const sourceCart = {
    id: "cart-1",
    currency: "USD",
    status: "active",
    subtotal_amount_minor: 2_000,
    lines: [
      {
        product_id: "product-1",
        product_variant_id: "variant-1",
        product_title: "Trail pack",
        variant_title: "One size",
        quantity: 1,
        unit_price_amount_minor: 2_000,
        subtotal_amount_minor: 2_000,
        media: [],
      },
    ],
  };
  const nextCart = {
    ...sourceCart,
    id: "active-cart",
    status: "active",
    subtotal_amount_minor: 0,
    lines: [],
  };
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1")) {
        return jsonResponse(200, { data: sourceCart });
      }
      if (url.endsWith("/carts/cart-1/checkout")) {
        return jsonResponse(201, {
          data: {
            order_id: "00000000-0000-4000-8000-000000000001",
            order_number: "W-20260830-00000001",
            checkout_token: "checkout-token",
            client_action: {
              type: "stripe_checkout_embedded",
              public_key: "pk_test_stripe",
              client_token: "cs_test_secret",
            },
          },
        });
      }
      if (url.endsWith("/carts") && init.method === "POST") {
        return jsonResponse(201, { data: nextCart });
      }
      return jsonResponse(404, {
        error: { code: "cart_not_found", message: "not found" },
      });
    }) as unknown as typeof fetch,
  });

  const recordedCreations: unknown[] = [];
  client.recordCheckoutCreation = (input) => recordedCreations.push(input);

  const creation = await client.payments.createEmbeddedCheckoutWithCart("cart-1");

  assert.deepEqual(creation.data.source_cart, sourceCart);
  assert.deepEqual(creation.data.cart, nextCart);
  assert.deepEqual(recordedCreations, [{
    checkout: creation.data.checkout,
    source_cart: sourceCart,
  }]);
});

test("checkout creation is recorded even when acquiring the next Cart fails", async () => {
  const sourceCart = {
    id: "cart-1",
    currency: "USD",
    status: "active" as const,
    subtotal_amount_minor: 2_000,
    created_at: "2026-08-30T00:00:00Z",
    updated_at: "2026-08-30T00:00:00Z",
    lines: [],
  };
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1")) {
        return jsonResponse(200, { data: sourceCart });
      }
      if (url.endsWith("/carts/cart-1/checkout")) {
        return jsonResponse(201, {
          data: {
            order_id: "00000000-0000-4000-8000-000000000001",
            order_number: "W-20260830-00000001",
            checkout_token: "checkout-token",
            client_action: {
              type: "stripe_checkout_embedded",
              public_key: "pk_test_stripe",
              client_token: "cs_test_secret",
            },
          },
        });
      }
      assert.equal(init.method, "POST");
      return jsonResponse(503, {
        error: { code: "cart_unavailable", message: "try later" },
      });
    }) as unknown as typeof fetch,
  });
  const recordedCreations: unknown[] = [];
  client.recordCheckoutCreation = (input) => recordedCreations.push(input);

  await assert.rejects(
    () => client.payments.createEmbeddedCheckoutWithCart("cart-1"),
    (error: unknown) =>
      error instanceof ChaosApiError && error.code === "cart_unavailable",
  );
  assert.equal(recordedCreations.length, 1);
  assert.deepEqual(recordedCreations[0], {
    checkout: {
      order_id: "00000000-0000-4000-8000-000000000001",
      order_number: "W-20260830-00000001",
      checkout_token: "checkout-token",
      checkout_url:
        "https://shop.example.com/#order_id=00000000-0000-4000-8000-000000000001&checkout_token=checkout-token",
      client_action: {
        type: "stripe_checkout_embedded",
        public_key: "pk_test_stripe",
        client_token: "cs_test_secret",
      },
    },
    source_cart: sourceCart,
  });
});

test("checkout can hand off directly from a fresh Cart without another read or Cart rotation", async () => {
  const sourceCart = {
    id: "cart-1",
    currency: "USD",
    status: "active" as const,
    subtotal_amount_minor: 2_000,
    created_at: "2026-08-30T00:00:00Z",
    updated_at: "2026-08-30T00:00:00Z",
    lines: [
      {
        product_id: "product-1",
        product_variant_id: "variant-1",
        product_title: "Trail pack",
        variant_title: "One size",
        quantity: 1,
        unit_price_amount_minor: 2_000,
        subtotal_amount_minor: 2_000,
        media: [],
      },
    ],
  };
  const requests: Array<{ url: string; method: string | undefined }> = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({ url, method: init.method });
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1/checkout")) {
        return jsonResponse(201, {
          data: {
            order_id: "00000000-0000-4000-8000-000000000001",
            order_number: "W-20260830-00000001",
            checkout_token: "checkout-token",
            client_action: {
              type: "stripe_checkout_embedded",
              public_key: "pk_test_stripe",
              client_token: "cs_test_secret",
            },
          },
        });
      }
      return jsonResponse(404, {
        error: { code: "cart_not_found", message: "not found" },
      });
    }) as unknown as typeof fetch,
  });
  await client.acquireShopperToken();
  requests.length = 0;
  const recordedStarts: unknown[] = [];
  client.recordCheckoutCreation = (input) => recordedStarts.push(input);

  const start = await client.payments.createEmbeddedCheckoutFromCart(sourceCart);

  assert.deepEqual(requests, [
    {
      url: "https://shop.example.com/api/v1/carts/cart-1/checkout",
      method: "POST",
    },
  ]);
  assert.deepEqual(start.data.source_cart, sourceCart);
  assert.equal(start.data.checkout.client_action.client_token, "cs_test_secret");
  assert.deepEqual(recordedStarts, [start.data]);
});

test("payments reject a checkout response that is missing required fields", async () => {
  let sequence = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    randomUUID: () => `id-${++sequence}`,
    fetch: (async (url: string) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1")) {
        return jsonResponse(200, {
          data: {
            id: "cart-1",
            currency: "USD",
            subtotal_amount_minor: 2_000,
            lines: [],
          },
        });
      }
      return jsonResponse(201, { data: { order_number: "W-20260830-00000001" } });
    }) as unknown as typeof fetch,
  });

  await assert.rejects(
    client.payments.createEmbeddedCheckout("cart-1"),
    (error: unknown) =>
      error instanceof ChaosApiError &&
      error.status === 502 &&
      error.code === "invalid_checkout_response",
  );
});

test("checkout omits attribution when the browser has no source data", async () => {
  const requests: Array<{ url: string; body: string | undefined }> = [];
  let sequence = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    randomUUID: () => `id-${++sequence}`,
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({
        url,
        body: typeof init.body === "string" ? init.body : undefined,
      });
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1")) {
        return jsonResponse(200, {
          data: {
            id: "cart-1",
            currency: "USD",
            subtotal_amount_minor: 2_000,
            lines: [],
          },
        });
      }
      if (url.endsWith("/checkout")) {
        return jsonResponse(201, {
          data: {
            order_id: "00000000-0000-4000-8000-000000000001",
            order_number: "W-20260830-00000001",
            checkout_token: "checkout-token",
            client_action: {
              type: "stripe_checkout_embedded",
              public_key: "pk_test_stripe",
              client_token: "cs_test_secret",
            },
          },
        });
      }
      return jsonResponse(404, {
        error: { code: "cart_not_found", message: "not found" },
      });
    }) as unknown as typeof fetch,
  });

  await client.payments.createEmbeddedCheckout("cart-1");

  assert.deepEqual(JSON.parse(requests[2]?.body ?? "{}"), {
    payment_provider: "stripe",
  });
});

test("checkout reuses one idempotency key per cart so a retry cannot double-charge", async () => {
  let cartQuantity = 1;
  let sequence = 0;
  const idempotencyKeys: string[] = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    randomUUID: () => `key-${++sequence}`,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("shopper-token");
      }
      if (url.endsWith("/carts/cart-1") || url.endsWith("/carts/cart-2")) {
        return jsonResponse(200, {
          data: {
            id: url.endsWith("cart-2") ? "cart-2" : "cart-1",
            currency: "USD",
            subtotal_amount_minor: 2_000,
            lines: [
              {
                product_id: "product-1",
                product_variant_id: "variant-1",
                product_title: "Trail pack",
                variant_title: "One size",
                sku: "PACK-1",
                quantity: cartQuantity,
                unit_price_amount_minor: 2_000,
                subtotal_amount_minor: 2_000 * cartQuantity,
                media: [],
              },
            ],
          },
        });
      }
      idempotencyKeys.push(
        new Headers(init.headers).get("idempotency-key") ?? "",
      );
      return jsonResponse(201, {
        data: {
          order_id: "00000000-0000-4000-8000-000000000001",
          order_number: "W-20260830-55555555",
          checkout_token: "checkout-token",
          client_action: {
            type: "stripe_checkout_embedded",
            public_key: "pk_test_stripe",
            client_token: "cs_test_secret",
          },
        },
      });
    }) as unknown as typeof fetch,
  });

  const options = {};
  await client.payments.createEmbeddedCheckout("cart-1", options);
  await client.payments.createEmbeddedCheckout("cart-1", options);
  cartQuantity = 2;
  await client.payments.createEmbeddedCheckout("cart-1", options);
  await client.payments.createEmbeddedCheckout("cart-2", options);

  // Same cart id -> same key on every retry, regardless of cart contents; a
  // different cart id gets its own key.
  assert.equal(idempotencyKeys.length, 4);
  assert.equal(idempotencyKeys[0], idempotencyKeys[1]);
  assert.equal(idempotencyKeys[1], idempotencyKeys[2]);
  assert.notEqual(idempotencyKeys[2], idempotencyKeys[3]);
});

test("addLine after getOrCreate reuses the created cart without a GET", async () => {
  const calls: string[] = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      calls.push(`${init.method ?? "GET"} ${url}`);
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("tok");
      }
      if (url.endsWith("/carts") && init.method === "POST") {
        return jsonResponse(201, {
          data: {
            id: "cart-1",
            status: "active",
            currency: "USD",
            subtotal_amount_minor: 0,
            lines: [],
          },
        });
      }
      return jsonResponse(200, {
        data: {
          id: "cart-1",
          status: "active",
          currency: "USD",
          subtotal_amount_minor: 500,
          lines: [
            {
              product_id: "p-1",
              product_variant_id: "v-1",
              quantity: 1,
              unit_price_amount_minor: 500,
            },
          ],
        },
      });
    }) as unknown as typeof fetch,
  });

  const cart = await client.cart.getOrCreate();
  await client.cart.addLine(cart.data.id, "v-1", 1);

  assert.deepEqual(
    calls.map((entry) => entry.split(" ")[0]),
    ["POST", "POST", "PUT"],
  );
  assert.ok(
    !calls.some((entry) => entry.startsWith("GET")),
    "addLine must not issue GET /carts when it already holds a fresh cart",
  );
});

test("a second addLine on the same cart skips the GET and stacks quantity", async () => {
  let getCount = 0;
  let stored = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("tok");
      }
      if (init.method === "GET") {
        getCount += 1;
        return jsonResponse(200, {
          data: {
            id: "cart-1",
            status: "active",
            currency: "USD",
            subtotal_amount_minor: stored * 500,
            lines: stored
              ? [
                  {
                    product_id: "p",
                    product_variant_id: "v-1",
                    quantity: stored,
                    unit_price_amount_minor: 500,
                  },
                ]
              : [],
          },
        });
      }
      stored = JSON.parse(String(init.body)).quantity;
      return jsonResponse(200, {
        data: {
          id: "cart-1",
          status: "active",
          currency: "USD",
          subtotal_amount_minor: stored * 500,
          lines: [
            {
              product_id: "p",
              product_variant_id: "v-1",
              quantity: stored,
              unit_price_amount_minor: 500,
            },
          ],
        },
      });
    }) as unknown as typeof fetch,
  });

  const first = await client.cart.addLine("cart-1", "v-1", 1);
  assert.equal(first.data.lines[0]!.quantity, 1);
  const second = await client.cart.addLine("cart-1", "v-1", 2);
  assert.equal(second.data.lines[0]!.quantity, 3);
  assert.equal(getCount, 1, "only the first addLine reads the cart");
});

test("addLine re-reads the cart once the snapshot TTL has passed", async () => {
  let clock = 1_000;
  let getCount = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    cartSnapshotTtlMs: 10_000,
    now: () => clock,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("tok");
      }
      if (init.method === "GET") getCount += 1;
      return jsonResponse(200, {
        data: {
          id: "cart-1",
          status: "active",
          currency: "USD",
          subtotal_amount_minor: 0,
          lines: [],
        },
      });
    }) as unknown as typeof fetch,
  });

  await client.cart.addLine("cart-1", "v-1", 1); // cold -> GET #1
  clock += 5_000;
  await client.cart.addLine("cart-1", "v-1", 1); // within TTL -> no GET
  clock += 10_000;
  await client.cart.addLine("cart-1", "v-1", 1); // TTL elapsed -> GET #2

  assert.equal(getCount, 2);
});

test("cartSnapshotTtlMs 0 makes every mutation re-read the cart", async () => {
  let getCount = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    cartSnapshotTtlMs: 0,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("tok");
      }
      if (init.method === "GET") getCount += 1;
      return jsonResponse(200, {
        data: {
          id: "cart-1",
          status: "active",
          currency: "USD",
          subtotal_amount_minor: 0,
          lines: [],
        },
      });
    }) as unknown as typeof fetch,
  });

  await client.cart.addLine("cart-1", "v-1", 1);
  await client.cart.addLine("cart-1", "v-1", 1);
  assert.equal(getCount, 2);
});

test("resume resolves a returning shopper's active cart from the server", async () => {
  const storage = new MemoryStorage();
  const makeClient = (calls: string[]) =>
    new ChaosStorefrontClient({
      publishableKey: "public_test",
      baseUrl: "https://shop.example.com/api/v1",
      storage,
      fetch: (async (url: string, init: RequestInit) => {
        calls.push(`${init.method ?? "GET"} ${url}`);
        if (url.endsWith("/shopper/sessions")) {
          return shopperSessionResponse("tok");
        }
        return jsonResponse(init.method === "POST" ? 201 : 200, {
          data: {
            id: "cart-1",
            status: "active",
            currency: "USD",
            subtotal_amount_minor: 0,
            lines: [],
          },
        });
      }) as unknown as typeof fetch,
    });

  const firstLoad: string[] = [];
  const created = await makeClient(firstLoad).cart.resume();
  assert.equal(created.data.id, "cart-1");
  assert.ok(
    firstLoad.some(
      (entry) =>
        entry === "POST https://shop.example.com/api/v1/carts",
    ),
    "a newly issued shopper session creates its first cart directly",
  );

  const secondLoad: string[] = [];
  const resumed = await makeClient(secondLoad).cart.resume();
  assert.equal(resumed.data.id, "cart-1");
  assert.ok(
    secondLoad.includes("GET https://shop.example.com/api/v1/carts"),
    "a returning shopper asks the server for its current cart",
  );
  assert.ok(
    !secondLoad.some((entry) => entry.startsWith("POST https://shop.example.com/api/v1/carts")),
    "second load does not create a new cart",
  );
});

test("resume creates a cart when a returning shopper has no active cart", async () => {
  const storage = new MemoryStorage();
  const firstClient = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage,
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
  });
  firstClient.setShopperToken("tok");

  const calls: string[] = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage,
    fetch: (async (url: string, init: RequestInit) => {
      calls.push(`${init.method ?? "GET"} ${url}`);
      if (url.endsWith("/carts") && init.method === "GET") {
        return jsonResponse(404, {
          error: { code: "cart_not_found", message: "active cart not found" },
        });
      }
      return jsonResponse(201, {
        data: {
          id: "cart-2",
          status: "active",
          currency: "USD",
          subtotal_amount_minor: 0,
          lines: [],
        },
      });
    }) as unknown as typeof fetch,
  });

  const resumed = await client.cart.resume();
  assert.equal(resumed.data.id, "cart-2");
  assert.deepEqual(calls, [
    "GET https://shop.example.com/api/v1/carts",
    "POST https://shop.example.com/api/v1/carts",
  ]);
});

test("resume does not create a cart for an unrelated 404", async () => {
  let requestCount = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async () => {
      requestCount += 1;
      return jsonResponse(404, {
        error: { code: "not_found", message: "the route was not found" },
      });
    }) as unknown as typeof fetch,
  });
  client.setShopperToken("shopper-token");

  await assert.rejects(client.cart.resume(), (error: unknown) => {
    return error instanceof ChaosApiError && error.code === "not_found";
  });
  assert.equal(requestCount, 1);
});

test("concurrent warmup calls do one session and one cart round", async () => {
  let sessions = 0;
  let carts = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        sessions += 1;
        await new Promise((resolve) => setTimeout(resolve, 0));
        return shopperSessionResponse("tok");
      }
      if (url.endsWith("/carts") && init.method === "POST") {
        carts += 1;
        return jsonResponse(201, {
          data: {
            id: "cart-1",
            status: "active",
            currency: "USD",
            subtotal_amount_minor: 0,
            lines: [],
          },
        });
      }
      return jsonResponse(404, { error: { code: "cart_not_found", message: "x" } });
    }) as unknown as typeof fetch,
  });

  const [a, b] = await Promise.all([
    client.cart.warmup(),
    client.cart.warmup(),
  ]);
  assert.equal(a.data.id, "cart-1");
  assert.equal(b.data.id, "cart-1");
  assert.equal(sessions, 1);
  assert.equal(carts, 1);
});

test("addLine recovers from cart_not_active by rotating to a fresh cart", async () => {
  const puts: string[] = [];
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: new MemoryStorage(),
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("tok");
      }
      if (url.endsWith("/carts") && init.method === "POST") {
        return jsonResponse(201, {
          data: {
            id: "cart-new",
            status: "active",
            currency: "USD",
            subtotal_amount_minor: 0,
            lines: [],
          },
        });
      }
      if (init.method === "GET" && url.endsWith("/carts/cart-done")) {
        return jsonResponse(200, {
          data: {
            id: "cart-done",
            status: "completed",
            currency: "USD",
            subtotal_amount_minor: 0,
            lines: [],
          },
        });
      }
      if (init.method === "PUT") {
        puts.push(url);
        if (url.includes("/carts/cart-done/")) {
          return jsonResponse(409, {
            error: {
              code: "cart_not_active",
              message: "the Cart is no longer active",
            },
          });
        }
        return jsonResponse(200, {
          data: {
            id: "cart-new",
            status: "active",
            currency: "USD",
            subtotal_amount_minor: 500,
            lines: [
              {
                product_id: "p-1",
                product_variant_id: "v-1",
                quantity: 1,
                unit_price_amount_minor: 500,
              },
            ],
          },
        });
      }
      return jsonResponse(404, { error: { code: "cart_not_found", message: "x" } });
    }) as unknown as typeof fetch,
  });

  const result = await client.cart.addLine("cart-done", "v-1", 1);

  assert.equal(result.data.id, "cart-new");
  assert.equal(result.data.lines[0]!.quantity, 1);
  assert.deepEqual(
    puts.map((url) => new URL(url, "https://x").pathname),
    ["/api/v1/carts/cart-done/lines/v-1", "/api/v1/carts/cart-new/lines/v-1"],
  );
});

test("setLine surfaces cart_not_active if the fresh cart also rejects it", async () => {
  let posts = 0;
  const client = new ChaosStorefrontClient({
    publishableKey: "public_test",
    storage: null,
    fetch: (async (url: string, init: RequestInit) => {
      if (url.endsWith("/shopper/sessions")) {
        return shopperSessionResponse("tok");
      }
      if (url.endsWith("/carts") && init.method === "POST") {
        posts += 1;
        return jsonResponse(201, {
          data: {
            id: `cart-${posts}`,
            status: "active",
            currency: "USD",
            subtotal_amount_minor: 0,
            lines: [],
          },
        });
      }
      if (init.method === "GET") {
        return jsonResponse(200, {
          data: {
            id: "cart-0",
            status: "completed",
            currency: "USD",
            subtotal_amount_minor: 0,
            lines: [],
          },
        });
      }
      return jsonResponse(409, {
        error: { code: "cart_not_active", message: "the Cart is no longer active" },
      });
    }) as unknown as typeof fetch,
  });

  await assert.rejects(
    client.cart.setLine("cart-0", "v-1", { quantity: 2 }),
    (error: unknown) =>
      error instanceof ChaosApiError && error.code === "cart_not_active",
  );
  assert.equal(posts, 1, "recovery is attempted exactly once");
});
