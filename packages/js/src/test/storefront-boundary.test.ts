import assert from "node:assert/strict";
import test from "node:test";

import { CatalogResource } from "../resources/catalog.js";
import {
  addCartLine,
  createEmbeddedCheckoutFromRequest,
  createProductReviewFromRequest,
  createServerStorefrontClient,
  recordConfirmedPurchaseCapi,
  type StorefrontCookieJar,
} from "../ssr/server.js";
import {
  createStorefrontBrowserClient,
  StorefrontBrowserClient,
} from "../ssr/browser.js";
import { ChaosApiError } from "../errors.js";
import type { ChaosStorefrontAnalytics } from "../analytics.js";
import type { ChaosStorefrontClient } from "../client.js";
import type { Cart, Collection } from "../index.js";

function collection(handle: string): Collection {
  return {
    id: `collection-${handle}`,
    handle,
    title: handle,
    description: "",
    product_count: 0,
  };
}

function cart(lines: Cart["lines"]): Cart {
  return {
    id: "00000000-0000-4000-8000-000000000001",
    currency: "USD",
    status: "active",
    version: 1,
    lines,
    subtotal_amount_minor: lines.reduce(
      (total, line) => total + line.subtotal_amount_minor,
      0,
    ),
    created_at: "2026-08-28T00:00:00Z",
    updated_at: "2026-08-28T00:00:00Z",
  };
}

test("money and domain helpers are available from the public SDK", async () => {
  const sdk = await import("../index.js");

  assert.equal(sdk.toMajorUnits(1234, "USD"), 12.34);
  assert.equal(sdk.toMajorUnits(1234, "JPY"), 1234);
  assert.equal(sdk.toMinorUnits(12.345, "BHD"), 12345);
  assert.equal(sdk.getOrderConfirmationState("confirmed", "paid"), "confirmed");
  assert.equal(sdk.getOrderConfirmationState("cancelled", "expired"), "expired");
});

test("browser commerce bridge owns API paths, response envelopes, and mutation analytics", async () => {
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const recorded: string[] = [];
  const line = {
    product_id: "00000000-0000-4000-8000-000000000010",
    product_variant_id: "00000000-0000-4000-8000-000000000011",
    product_title: "Trail pack",
    variant_title: "One size",
    quantity: 2,
    unit_price_amount_minor: 9900,
    subtotal_amount_minor: 19800,
    media: [],
  } satisfies Cart["lines"][number];
  const mutation = {
    cart: cart([line]),
    product_variant_id: line.product_variant_id,
    previous_quantity: 1,
    new_quantity: 2,
    removed: false,
  };
  const analytics = {
    recordCartMutation: () => recorded.push("cart"),
    recordCheckoutCreation: () => recorded.push("checkout"),
  };
  const browser = createStorefrontBrowserClient({
    baseUrl: "/api",
    analytics: analytics as unknown as ChaosStorefrontAnalytics,
    fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
      requests.push({ url: String(input), init: init ?? {} });
      return new Response(JSON.stringify({ data: mutation }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }) as typeof fetch,
  });

  const result = await browser.cart.addLine(line.product_variant_id, 1);

  assert.deepEqual(result, mutation);
  assert.equal(requests[0]?.url, "/api/cart/line-items");
  assert.deepEqual(JSON.parse(String(requests[0]?.init.body)), {
    variant_id: line.product_variant_id,
    quantity: 1,
  });
  assert.deepEqual(recorded, ["cart"]);
  assert.equal((requests[0]?.init.credentials), "same-origin");
  assert.ok(browser instanceof StorefrontBrowserClient);
});

test("browser checkout bridge forwards shared checkout options", async () => {
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const recorded: string[] = [];
  const creation = {
    checkout: {
      order_number: "W-20260830-00000041",
      client_action: {
        type: "mount_embedded_checkout" as const,
        public_key: "pk_test_store",
        client_token: "cs_test_token",
      },
    },
    source_cart: cart([]),
    cart: cart([]),
  };
  const browser = createStorefrontBrowserClient({
    analytics: {
      recordCartMutation: () => undefined,
      recordCheckoutCreation: () => recorded.push("checkout"),
      recordPurchase: () => recorded.push("purchase"),
    } as unknown as ChaosStorefrontAnalytics,
    fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
      requests.push({ url: String(input), init: init ?? {} });
      return new Response(JSON.stringify({ data: creation }), { status: 201 });
    }) as typeof fetch,
  });

  const result = await browser.checkout.createEmbeddedCheckout({
    returnUrl: "https://shop.example/checkout/confirmation",
    email: "shopper@example.com",
  });

  assert.deepEqual(result, creation);
  assert.equal(requests[0]?.url, "/api/checkout");
  assert.deepEqual(JSON.parse(String(requests[0]?.init.body)), {
    returnUrl: "https://shop.example/checkout/confirmation",
    email: "shopper@example.com",
  });
  browser.orders.recordPurchase({
    orderId: "00000000-0000-4000-8000-000000000041",
    valueMinor: 9900,
    currency: "USD",
    items: [
      {
        productId: "00000000-0000-4000-8000-000000000042",
        productVariantId: "00000000-0000-4000-8000-000000000043",
        quantity: 1,
        priceMinor: 9900,
      },
    ],
  });
  assert.deepEqual(recorded, ["checkout", "purchase"]);
});

test("browser catalog bridge forwards product reads and records Search/ViewContent", async () => {
  const requests: string[] = [];
  const recorded: Array<[string, unknown]> = [];
  const product = { id: "00000000-0000-4000-8000-000000000050", handle: "trail-pack" };
  const browser = createStorefrontBrowserClient({
    analytics: {
      search: (input: unknown) => recorded.push(["search", input]),
      viewContent: (input: unknown) => recorded.push(["view_content", input]),
    } as unknown as ChaosStorefrontAnalytics,
    fetch: (async (input: RequestInfo | URL) => {
      const url = String(input);
      requests.push(url);
      if (url.endsWith("/products?q=shoes")) {
        return new Response(
          JSON.stringify({ data: [product], meta: { page: { has_more: false } } }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({ data: product }), { status: 200 });
    }) as typeof fetch,
  });

  await browser.catalog.listProducts({ q: "shoes" });
  await browser.catalog.getProduct("trail-pack");

  assert.equal(requests[0], "/api/products?q=shoes");
  assert.equal(requests[1], "/api/products/trail-pack");
  assert.deepEqual(recorded, [
    ["search", { query: "shoes" }],
    ["view_content", { productId: product.id }],
  ]);
});

test("server checkout bridge creates a new Cart when the source Cart is terminal", async () => {
  const writes = new Map<string, string>();
  const cookies: StorefrontCookieJar = {
    get: (name) => {
      const value = writes.get(name);
      return value === undefined ? undefined : { value };
    },
    set: (name, value) => writes.set(name, value),
  };
  writes.set("chaos_cart_id", "source-cart");
  let usedCreate = false;
  const response = {
    data: {
      checkout: {
        order_number: "W-20260830-00000061",
        client_action: {
          type: "mount_embedded_checkout" as const,
          public_key: "pk_test_store",
          client_token: "cs_test_token",
        },
      },
      source_cart: { ...cart([]), id: "source-cart" },
      cart: { ...cart([]), id: "active-cart" },
    },
  };
  const client = {
    getShopperToken: () => "shopper-token",
    cart: {
      getOrCreate: async (cartId?: string) => {
        assert.equal(cartId, "source-cart");
        usedCreate = true;
        return { data: { ...cart([]), id: "active-cart" } };
      },
    },
    payments: {
      createEmbeddedCheckoutWithCart: async (cartId: string) => {
        if (cartId === "source-cart") {
          throw new ChaosApiError(409, "cart_not_active", "the Cart is no longer active");
        }
        assert.equal(cartId, "active-cart");
        return response;
      },
    },
  } as unknown as ChaosStorefrontClient;

  const result = await createEmbeddedCheckoutFromRequest(
    client,
    cookies,
    new Request("https://shop.example/checkout", {
      method: "POST",
      body: JSON.stringify({
        returnUrl: "https://shop.example/checkout/confirmation",
      }),
      headers: { "Content-Type": "application/json" },
    }),
  );

  assert.equal(usedCreate, true);
  assert.deepEqual(result, response);
  assert.equal(writes.get("chaos_cart_id"), "active-cart");
});

test("server checkout bridge retries the same Cart after a lost response", async () => {
  const writes = new Map<string, string>([["chaos_cart_id", "source-cart"]]);
  const cookies: StorefrontCookieJar = {
    get: (name) => {
      const value = writes.get(name);
      return value === undefined ? undefined : { value };
    },
    set: (name, value) => writes.set(name, value),
  };
  const checkout = {
    order_number: "W-20260830-00000071",
    client_action: {
      type: "mount_embedded_checkout" as const,
      public_key: "pk_test_store",
      client_token: "cs_test_token",
    },
  };
  let retriedCart: string | undefined;
  const client = {
    getShopperToken: () => "shopper-token",
    payments: {
      createEmbeddedCheckoutWithCart: async (cartId: string, options: { returnUrl: string }) => {
        assert.equal(cartId, "source-cart");
        assert.equal(options.returnUrl, "https://shop.example/checkout/confirmation");
        retriedCart = cartId;
        return {
          data: {
            checkout,
            source_cart: { ...cart([]), id: "source-cart" },
            cart: { ...cart([]), id: "active-cart" },
          },
        };
      },
    },
  } as unknown as ChaosStorefrontClient;

  const result = await createEmbeddedCheckoutFromRequest(
    client,
    cookies,
    new Request("https://shop.example/checkout", {
      method: "POST",
      body: JSON.stringify({
        returnUrl: "https://shop.example/checkout/confirmation",
      }),
      headers: { "Content-Type": "application/json" },
    }),
  );

  assert.equal(retriedCart, "source-cart");
  assert.equal(result.data.checkout.order_number, checkout.order_number);
  assert.equal(result.data.cart.id, "active-cart");
  assert.equal(writes.get("chaos_cart_id"), "active-cart");
});

test("server review and cart adapters own request parsing and cookie persistence", async () => {
  const submitted: { productId: string; payload: unknown }[] = [];
  const reviewClient = {
    reviews: {
      submit: async (productId: string, payload: unknown) => {
        submitted.push({ productId, payload });
      },
    },
  } as unknown as ChaosStorefrontClient;

  await createProductReviewFromRequest(
    reviewClient,
    new Request("https://shop.example/review", {
      method: "POST",
      body: JSON.stringify({
        rating: 5,
        title: "Excellent",
        content: "Very comfortable",
        author_name: "Ada",
        author_email: "ada@example.com",
      }),
      headers: { "Content-Type": "application/json" },
    }),
    "00000000-0000-4000-8000-000000000020",
  );

  assert.deepEqual(submitted, [
    {
      productId: "00000000-0000-4000-8000-000000000020",
      payload: {
        rating: 5,
        title: "Excellent",
        content: "Very comfortable",
        author_name: "Ada",
        author_email: "ada@example.com",
      },
    },
  ]);

  let current = cart([]);
  const writes = new Map<string, string>();
  const cookies: StorefrontCookieJar = {
    get: (name) => {
      const value = writes.get(name);
      return value === undefined ? undefined : { value };
    },
    set: (name, value) => writes.set(name, value),
  };
  const cartClient = {
    cart: {
      getOrCreate: async () => ({ data: current }),
      addLine: async (_cartId: string, variantId: string, quantity: number) => {
        current = cart([
          {
            product_id: "00000000-0000-4000-8000-000000000030",
            product_variant_id: variantId,
            product_title: "Trail pack",
            variant_title: "One size",
            quantity,
            unit_price_amount_minor: 1000,
            subtotal_amount_minor: quantity * 1000,
            media: [],
          },
        ]);
        return { data: current };
      },
    },
  } as unknown as ChaosStorefrontClient;

  const mutation = await addCartLine(
    cartClient,
    cookies,
    {
      variantId: "00000000-0000-4000-8000-000000000031",
      quantity: 2,
    },
  );

  assert.equal(mutation.new_quantity, 2);
  assert.equal(writes.get("chaos_cart_id"), current.id);
});

test("collection cache is isolated by store and shares in-flight reads", async () => {
  let firstCalls = 0;
  let secondCalls = 0;
  const makeClient = (publishableKey: string, data: Collection[]) =>
    ({
      baseUrl: "https://chaos.example/api/v1",
      publishableKey,
      request: async () => {
        if (publishableKey === "pk-cache-a") firstCalls += 1;
        else secondCalls += 1;
        return { data, meta: { page: { has_more: false } } };
      },
    }) as unknown as ChaosStorefrontClient;

  const first = new CatalogResource(makeClient("pk-cache-a", [collection("a")]));
  const second = new CatalogResource(makeClient("pk-cache-b", [collection("b")]));
  const [firstResult, firstAgain] = await Promise.all([
    first.listCollectionsCached({ limit: 100 }),
    first.listCollectionsCached({ limit: 100 }),
  ]);
  const secondResult = await second.listCollectionsCached({ limit: 100 });

  assert.deepEqual(firstResult, firstAgain);
  assert.deepEqual(firstResult.map((item) => item.handle), ["a"]);
  assert.deepEqual(secondResult.map((item) => item.handle), ["b"]);
  assert.equal(firstCalls, 1);
  assert.equal(secondCalls, 1);
});

function jsonResponse(status: number, body: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: new Headers({ "content-type": "application/json" }),
    json: async () => body,
  } as unknown as Response;
}

test("addCartLine sends Meta CAPI and shares the event ID with the mutation result", async () => {
  const writes = new Map<string, string>();
  writes.set("_fbc", "fb.1.1700000000000.click");
  writes.set("_fbp", "fb.1.1700000000000.browser");
  const cookies: StorefrontCookieJar = {
    get: (name) => {
      const value = writes.get(name);
      return value === undefined ? undefined : { value };
    },
    set: (name, value) => writes.set(name, value),
  };

  let cartState = cart([]);
  const storefrontFetch = (async (url: string, init: RequestInit = {}) => {
    const method = init.method ?? "GET";
    if (url.endsWith("/shopper/sessions")) {
      return jsonResponse(201, { data: { shopper_token: "shopper-token" } });
    }
    if (url.endsWith("/carts") && method === "POST") {
      return jsonResponse(201, { data: cartState });
    }
    if (/\/carts\/[^/]+$/.test(url) && method === "GET") {
      return jsonResponse(200, { data: cartState });
    }
    if (/\/carts\/[^/]+\/lines\//.test(url) && method === "PUT") {
      const body = JSON.parse(String(init.body)) as { quantity: number };
      cartState = cart([
        {
          product_id: "00000000-0000-4000-8000-000000000030",
          product_variant_id: "00000000-0000-4000-8000-000000000031",
          product_title: "Trail pack",
          variant_title: "One size",
          quantity: body.quantity,
          unit_price_amount_minor: 1_000,
          subtotal_amount_minor: body.quantity * 1_000,
          media: [],
        },
      ]);
      return jsonResponse(200, { data: cartState });
    }
    return jsonResponse(404, { error: { code: "not_found", message: "not found" } });
  }) as unknown as typeof fetch;

  const capiRequests: Array<Record<string, unknown>> = [];
  const client = createServerStorefrontClient({
    publishableKey: "public_test",
    baseUrl: "https://shop.example.com/api/v1",
    cookies,
    fetch: storefrontFetch,
    capi: {
      meta: {
        accessToken: "capi-token",
        pixelId: "pixel-1",
        fetch: (async (_url: string, init: RequestInit) => {
          capiRequests.push(JSON.parse(String(init.body)));
          return jsonResponse(200, { events_received: 1 });
        }) as unknown as typeof fetch,
      },
    },
  });

  const mutation = await addCartLine(client, cookies, {
    variantId: "00000000-0000-4000-8000-000000000031",
    quantity: 2,
  });

  assert.equal(typeof mutation.event_id, "string");
  assert.equal(capiRequests.length, 1);
  const capiEvent = (capiRequests[0]!.data as Array<Record<string, unknown>>)[0]!;
  assert.equal(capiEvent.event_id, mutation.event_id);
  assert.equal(capiEvent.event_name, "AddToCart");
  const userData = capiEvent.user_data as Record<string, unknown>;
  assert.equal(userData.fbc, "fb.1.1700000000000.click");
  assert.equal(userData.fbp, "fb.1.1700000000000.browser");
});

test("recordConfirmedPurchaseCapi uses the same order-derived event ID as the browser recordPurchase() call", async () => {
  const cookies: StorefrontCookieJar = { get: () => undefined, set: () => {} };
  const capiRequests: Array<Record<string, unknown>> = [];
  const client = createServerStorefrontClient({
    publishableKey: "public_test",
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
    capi: {
      meta: {
        accessToken: "capi-token",
        pixelId: "pixel-1",
        fetch: (async (_url: string, init: RequestInit) => {
          capiRequests.push(JSON.parse(String(init.body)));
          return jsonResponse(200, { events_received: 1 });
        }) as unknown as typeof fetch,
      },
    },
  });

  await recordConfirmedPurchaseCapi(client, cookies, {
    id: "00000000-0000-4000-8000-000000000099",
    status: "confirmed",
    payment_status: "paid",
    currency: "USD",
    total_amount_minor: 2_000,
    lines: [
      {
        product_id: "00000000-0000-4000-8000-000000000091",
        product_variant_id: "00000000-0000-4000-8000-000000000092",
        product_title: "Trail pack",
        variant_title: "One size",
        quantity: 1,
        unit_price_amount_minor: 2_000,
        subtotal_amount_minor: 2_000,
      },
    ],
  });

  assert.equal(capiRequests.length, 1);
  const event = (capiRequests[0]!.data as Array<Record<string, unknown>>)[0]!;
  assert.equal(event.event_id, "00000000-0000-4000-8000-000000000099");
  assert.equal(event.event_name, "Purchase");
});

test("recordConfirmedPurchaseCapi is a no-op for an order that is not confirmed and paid", async () => {
  const cookies: StorefrontCookieJar = { get: () => undefined, set: () => {} };
  let capiCalls = 0;
  const client = createServerStorefrontClient({
    publishableKey: "public_test",
    fetch: (async () => jsonResponse(200, { data: {} })) as unknown as typeof fetch,
    capi: {
      meta: {
        accessToken: "capi-token",
        pixelId: "pixel-1",
        fetch: (async () => {
          capiCalls += 1;
          return jsonResponse(200, { events_received: 1 });
        }) as unknown as typeof fetch,
      },
    },
  });

  await recordConfirmedPurchaseCapi(client, cookies, {
    id: "00000000-0000-4000-8000-000000000098",
    status: "pending",
    payment_status: "pending",
    currency: "USD",
    total_amount_minor: 2_000,
    lines: [],
  });

  assert.equal(capiCalls, 0);
});
