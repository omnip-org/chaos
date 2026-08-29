import assert from "node:assert/strict";
import test from "node:test";

import { CatalogResource } from "../resources/catalog.js";
import {
  addCartLine,
  createEmbeddedCheckoutFromRequest,
  createProductReviewFromRequest,
  type StorefrontCookieJar,
} from "../server.js";
import {
  createStorefrontBrowserClient,
  StorefrontBrowserClient,
} from "../browser.js";
import type { ChaosStorefrontAnalytics } from "../analytics.js";
import type {
  Cart,
  ChaosStorefrontClient,
  Collection,
} from "../index.js";

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
    price_list_id: "00000000-0000-4000-8000-000000000002",
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
});

test("browser commerce bridge owns API paths, response envelopes, and mutation analytics", async () => {
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const recorded: string[] = [];
  const line = {
    product_id: "00000000-0000-4000-8000-000000000010",
    product_variant_id: "00000000-0000-4000-8000-000000000011",
    product_title: "Trail pack",
    variant_title: "One size",
    track_inventory: false,
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
  assert.equal(
    (requests[0]?.init.body as URLSearchParams).get("variant_id"),
    line.product_variant_id,
  );
  assert.equal((requests[0]?.init.body as URLSearchParams).get("quantity"), "1");
  assert.deepEqual(recorded, ["cart"]);
  assert.equal((requests[0]?.init.credentials), "same-origin");
  assert.ok(browser instanceof StorefrontBrowserClient);
});

test("browser checkout bridge forwards shared checkout options", async () => {
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const recorded: string[] = [];
	const creation = {
		checkout: {
			checkout_attempt_id: "00000000-0000-4000-8000-000000000040",
			order_id: "00000000-0000-4000-8000-000000000041",
			source_cart_id: "00000000-0000-4000-8000-000000000042",
			successor_cart_id: "00000000-0000-4000-8000-000000000043",
			status: "open" as const,
			expires_at: "2026-08-29T12:00:00Z",
      client_action: {
        type: "mount_embedded_checkout" as const,
        public_key: "pk_test_store",
        client_token: "cs_test_token",
      },
    },
    cart: cart([]),
  };
  const browser = createStorefrontBrowserClient({
    analytics: {
      recordCartMutation: () => undefined,
      recordCheckoutCreation: () => recorded.push("checkout"),
      purchase: () => recorded.push("purchase"),
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
    orderId: creation.checkout.order_id,
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

test("browser checkout bridge resumes a persisted attempt without creating another one", async () => {
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const creation = {
    checkout: {
      checkout_attempt_id: "00000000-0000-4000-8000-000000000050",
      order_id: "00000000-0000-4000-8000-000000000051",
      source_cart_id: "00000000-0000-4000-8000-000000000052",
      successor_cart_id: "00000000-0000-4000-8000-000000000053",
      status: "open" as const,
      expires_at: "2026-08-29T12:00:00Z",
      client_action: {
        type: "mount_embedded_checkout" as const,
        public_key: "pk_test_store",
        client_token: "cs_test_token",
      },
    },
    cart: cart([]),
  };
  const attempts = [
    {
      id: creation.checkout.checkout_attempt_id,
      order_id: creation.checkout.order_id,
      source_cart_id: creation.checkout.source_cart_id,
      successor_cart_id: creation.checkout.successor_cart_id,
      status: "open" as const,
      expires_at: creation.checkout.expires_at,
      created_at: "2026-08-29T11:30:00Z",
      updated_at: "2026-08-29T11:31:00Z",
    },
  ];
  const browser = createStorefrontBrowserClient({
    fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = { url: String(input), init: init ?? {} };
      requests.push(request);
      if (request.url === "/api/checkout-attempts") {
        return new Response(JSON.stringify({ data: attempts }), { status: 200 });
      }
      assert.equal(request.url, "/api/checkout/resume");
      assert.equal(request.init.method, "POST");
      assert.deepEqual(JSON.parse(String(request.init.body)), {
        checkoutAttemptId: creation.checkout.checkout_attempt_id,
      });
      return new Response(JSON.stringify({ data: creation }), { status: 200 });
    }) as typeof fetch,
  });

  assert.deepEqual(await browser.checkout.listCheckoutAttempts(), attempts);
  assert.deepEqual(
    await browser.checkout.resumeEmbeddedCheckout(
      creation.checkout.checkout_attempt_id,
    ),
    creation,
  );
  assert.equal(requests[0]?.init.credentials, "same-origin");
  assert.equal(requests[1]?.init.credentials, "same-origin");
});

test("server checkout bridge keeps a pending source cart addressable after a lost response", async () => {
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
        checkout_attempt_id: "00000000-0000-4000-8000-000000000060",
        order_id: "00000000-0000-4000-8000-000000000061",
        source_cart_id: "source-cart",
        successor_cart_id: "successor-cart",
        status: "open" as const,
        expires_at: "2026-08-29T12:00:00Z",
        client_action: {
          type: "mount_embedded_checkout" as const,
          public_key: "pk_test_store",
          client_token: "cs_test_token",
        },
      },
      cart: { ...cart([]), id: "successor-cart" },
    },
  };
  const client = {
    cart: {
      get: async () => ({
        data: { ...cart([]), id: "source-cart", status: "checkout_pending" },
      }),
      getOrCreate: async () => {
        usedCreate = true;
        return { data: cart([]) };
      },
    },
    payments: {
      createEmbeddedCheckoutWithCart: async (cartId: string) => {
        assert.equal(cartId, "source-cart");
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

  assert.equal(usedCreate, false);
  assert.deepEqual(result, response);
  assert.equal(writes.get("chaos_cart_id"), "successor-cart");
});

test("server review and cart adapters own form parsing and cookie persistence", async () => {
  const submitted: { productId: string; payload: unknown }[] = [];
  const reviewClient = {
    reviews: {
      submit: async (productId: string, payload: unknown) => {
        submitted.push({ productId, payload });
      },
    },
  } as unknown as ChaosStorefrontClient;
  const reviewBody = new URLSearchParams({
    rating: "5",
    title: "Excellent",
    content: "Very comfortable",
    author_name: "Ada",
    author_email: "ada@example.com",
  });

  await createProductReviewFromRequest(
    reviewClient,
    new Request("https://shop.example/review", {
      method: "POST",
      body: reviewBody,
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
            track_inventory: false,
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
      baseUrl: "https://chaos.example/storefront/v1",
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
