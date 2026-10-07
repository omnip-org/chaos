import assert from "node:assert/strict";
import test from "node:test";
import { ChaosStorefrontAnalytics } from "../events/browser.js";
import type { EmbeddedCheckoutCreation } from "../types.js";

function harness(
  options: {
    search?: string;
    cookie?: string;
    providers?: {
      metaPixel?: { pixelId: string };
      ga4?: { measurementId: string };
    };
  } = {},
) {
  const time = Date.parse("2026-08-16T00:00:00Z");
  let sequence = 0;
  const scripts: Array<{ id: string; src: string; async: boolean }> = [];
  const location = {
    search: options.search ?? "?fbclid=fb-secret&gclid=g-secret",
    protocol: "https:",
  };
  const document = {
    cookie: options.cookie ?? "",
    location,
    getElementById: (id: string) => scripts.find((script) => script.id === id),
    createElement: () => ({ id: "", src: "", async: false }),
    head: {
      appendChild: (script: { id: string; src: string; async: boolean }) =>
        scripts.push(script),
    },
  };
  const window = {};
  const analytics = new ChaosStorefrontAnalytics({
    document: document as unknown as Document,
    window: window as unknown as Window & typeof globalThis,
    now: () => time,
    randomUUID: () =>
      `00000000-0000-4000-8000-${String(++sequence).padStart(12, "0")}`,
    ...(options.providers ? { providers: options.providers } : {}),
  });
  return { analytics, document, window, scripts };
}

function ga4Calls(window: unknown): unknown[][] {
  return (window as { dataLayer: unknown[][] }).dataLayer.filter(
    (call) => call[0] === "event",
  );
}

function ga4ConfigCalls(window: unknown): unknown[][] {
  return (window as { dataLayer: unknown[][] }).dataLayer.filter(
    (call) => call[0] === "config",
  );
}

function fbqCalls(window: unknown): unknown[][] {
  return (window as { fbq: { queue: unknown[][] } }).fbq.queue.filter(
    (call) => call[0] === "track",
  );
}

/** Advanced-matching re-inits only — excludes the plain `init(pixelId)` from `startMeta`. */
function fbqAdvancedMatchingCalls(window: unknown): unknown[][] {
  return (window as { fbq: { queue: unknown[][] } }).fbq.queue.filter(
    (call) => call[0] === "init" && call.length > 2,
  );
}

async function waitForAdvancedMatchingCalls(window: unknown): Promise<unknown[][]> {
  const deadline = Date.now() + 1_000;
  while (Date.now() < deadline) {
    const calls = fbqAdvancedMatchingCalls(window);
    if (calls.length > 0) return calls;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  return fbqAdvancedMatchingCalls(window);
}

test("keeps a fresh _fbc cookie in sync with the current fbclid", () => {
  const environment = harness({
    cookie: "_fbc=fb.1.1.old-click",
    search: "?fbclid=current-click",
  });
  const expectedFbc = `fb.1.${Date.parse("2026-08-16T00:00:00Z")}.current-click`;
  assert.match(
    environment.document.cookie,
    new RegExp(`_fbc=${encodeURIComponent(expectedFbc)}`),
  );
});

test("reuses the _fbc cookie for the current Meta click", () => {
  const cookie = "_fbc=fb.1.1234567890123.current-click";
  const environment = harness({
    cookie,
    search: "?fbclid=current-click",
  });
  assert.equal(environment.document.cookie, cookie);
});

test("retains a long Meta click identifier within the attribution bound", () => {
  const fbclid = "x".repeat(2_000);
  const environment = harness({ search: `?fbclid=${fbclid}` });
  const expectedFbc = `fb.1.${Date.parse("2026-08-16T00:00:00Z")}.${fbclid}`;
  assert.match(
    environment.document.cookie,
    new RegExp(`_fbc=${encodeURIComponent(expectedFbc)}`),
  );
});

test("does not resurrect a stale fbclid without a current one", () => {
  const environment = harness({
    cookie: "",
    search: "",
  });
  assert.doesNotMatch(environment.document.cookie, /_fbc=/);
});

test("keeps one stable provider event identity", () => {
  const environment = harness({
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  const eventId = environment.analytics.viewContent(
    "00000000-0000-4000-8000-000000000200",
  );
  const metaTrack = fbqCalls(environment.window).find(
    (call) => call[1] === "ViewContent",
  );
  assert.deepEqual(metaTrack?.[3], { eventID: eventId });
  const ga4 = ga4Calls(environment.window).find(
    (call) => call[1] === "view_item",
  );
  assert.deepEqual((ga4?.[2] as { items: unknown[] }).items, [
    {
      item_id: "00000000-0000-4000-8000-000000000200",
    },
  ]);
});

test("recordAddToCart forwards a repeated explicit event ID", () => {
  const environment = harness({
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  const explicitEventId = "00000000-0000-4000-8000-000000000321";
  const input = {
    cartId: "00000000-0000-4000-8000-000000000001",
    productId: "00000000-0000-4000-8000-000000000002",
    productVariantId: "00000000-0000-4000-8000-000000000003",
    itemName: "Running shoe",
    itemVariant: "Blue / 42",
    quantity: 1,
    priceMinor: 1_000,
    valueMinor: 1_000,
    currency: "usd",
  };

  const firstId = environment.analytics.recordAddToCart(input, explicitEventId);
  assert.equal(firstId, explicitEventId);
  const metaTrack = fbqCalls(environment.window).find(
    (call) => call[1] === "AddToCart",
  );
  assert.deepEqual(metaTrack?.[3], { eventID: explicitEventId });
  const ga4 = ga4Calls(environment.window).find(
    (call) => call[1] === "add_to_cart",
  );
  assert.deepEqual((ga4?.[2] as { items: unknown[] }).items, [
    {
      item_id: "00000000-0000-4000-8000-000000000002",
      item_name: "Running shoe",
      item_variant: "Blue / 42",
      quantity: 1,
      price: 10,
    },
  ]);

  const secondId = environment.analytics.recordAddToCart(input, explicitEventId);
  assert.equal(secondId, explicitEventId);
  assert.equal(
    fbqCalls(environment.window).filter((call) => call[1] === "AddToCart").length,
    2,
  );
  assert.equal(
    ga4Calls(environment.window).filter((call) => call[1] === "add_to_cart")
      .length,
    2,
  );
});

test("high-level commerce methods project canonical event properties", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });

  const eventId = environment.analytics.recordAddToCart({
    cartId: "00000000-0000-4000-8000-000000000001",
    productId: "00000000-0000-4000-8000-000000000002",
    productVariantId: "00000000-0000-4000-8000-000000000003",
    quantity: 2,
    priceMinor: 649,
    valueMinor: 1_298,
    currency: "usd",
  });

  const metaTrack = fbqCalls(environment.window).find(
    (call) => call[1] === "AddToCart",
  );
  assert.deepEqual(metaTrack?.[3], { eventID: eventId });
  assert.deepEqual(metaTrack?.[2], {
    content_ids: ["00000000-0000-4000-8000-000000000002"],
    content_type: "product",
    value: 12.98,
    currency: "USD",
    contents: [
      { id: "00000000-0000-4000-8000-000000000002", quantity: 2, item_price: 6.49 },
    ],
    num_items: 2,
  });
});

test("recordCartMutation mints a browser-owned event ID", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  const returnedId = environment.analytics.recordCartMutation({
    cart: {
      id: "00000000-0000-4000-8000-000000000030",
      currency: "USD",
      status: "active",
      lines: [
        {
          product_id: "00000000-0000-4000-8000-000000000031",
          product_variant_id: "00000000-0000-4000-8000-000000000032",
          product_title: "T",
          variant_title: "T",
          quantity: 2,
          unit_price_amount_minor: 500,
          subtotal_amount_minor: 1_000,
          media: [],
        },
      ],
      subtotal_amount_minor: 1_000,
      created_at: "2026-08-16T00:00:00Z",
      updated_at: "2026-08-16T00:00:00Z",
    },
    product_variant_id: "00000000-0000-4000-8000-000000000032",
    previous_quantity: 0,
    new_quantity: 2,
    removed: false,
  });
  assert.equal(returnedId, "00000000-0000-4000-8000-000000000001");
  const metaTrack = fbqCalls(environment.window).find(
    (call) => call[1] === "AddToCart",
  );
  assert.deepEqual(metaTrack?.[3], { eventID: returnedId });
});

test("records begin_checkout with the standard GA4 ecommerce fields", () => {
  const environment = harness({
    providers: { ga4: { measurementId: "G-TEST1234" } },
  });
  environment.analytics.recordInitiateCheckout({
    cartId: "00000000-0000-4000-8000-000000000011",
    valueMinor: 2_000,
    currency: "usd",
    items: [
      {
        productId: "00000000-0000-4000-8000-000000000012",
        productVariantId: "00000000-0000-4000-8000-000000000013",
        quantity: 1,
        priceMinor: 2_000,
      },
    ],
  });
  const call = ga4Calls(environment.window).find(
    (entry) => entry[1] === "begin_checkout",
  );
  const parameters = call?.[2] as Record<string, unknown>;
  assert.equal(parameters.value, 20);
  assert.equal(parameters.currency, "USD");
  assert.equal(parameters.transaction_id, undefined);
  assert.equal(parameters.event_id, undefined);
});

test("attributes server checkout creation to the source Cart", () => {
  const environment = harness({
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  const checkoutCreation: EmbeddedCheckoutCreation = {
    checkout: {
      order_id: "00000000-0000-4000-8000-000000000001",
      order_number: "W-20260830-7K4M9Q2D",
      checkout_token: "checkout-token",
      checkout_url:
        "https://shop.example.com/checkout#order_id=00000000-0000-4000-8000-000000000001&checkout_token=checkout-token",
      client_action: {
        type: "stripe_checkout_embedded",
        public_key: "pk_test_stripe",
        client_token: "cs_test_secret",
      },
    },
    source_cart: {
      id: "00000000-0000-4000-8000-000000000021",
      currency: "USD",
      status: "locked",
      lines: [
        {
          product_id: "00000000-0000-4000-8000-000000000024",
          product_variant_id: "00000000-0000-4000-8000-000000000025",
          product_title: "Test product",
          variant_title: "Test variant",
          quantity: 1,
          unit_price_amount_minor: 2_000,
          subtotal_amount_minor: 2_000,
          media: [],
        },
      ],
      subtotal_amount_minor: 2_000,
      created_at: "2026-08-16T00:00:00Z",
      updated_at: "2026-08-16T00:00:00Z",
    },
    cart: {
      id: "00000000-0000-4000-8000-000000000026",
      currency: "USD",
      status: "active",
      lines: [],
      subtotal_amount_minor: 0,
      created_at: "2026-08-16T00:00:00Z",
      updated_at: "2026-08-16T00:00:00Z",
    },
  };
  const eventId = environment.analytics.recordCheckoutCreation(checkoutCreation);
  const duplicateId = environment.analytics.recordCheckoutCreation(checkoutCreation);
  const call = ga4Calls(environment.window).find(
    (entry) => entry[1] === "begin_checkout",
  );
  const parameters = call?.[2] as Record<string, unknown>;
  assert.equal(eventId, "00000000-0000-4000-8000-000000000001");
  assert.equal(parameters.transaction_id, undefined);
  assert.equal(parameters.event_id, undefined);
  assert.equal(duplicateId, "00000000-0000-4000-8000-000000000001");
  assert.equal(
    ga4Calls(environment.window).filter((entry) => entry[1] === "begin_checkout")
      .length,
    2,
  );
  const items = parameters.items as Array<Record<string, unknown>>;
  assert.equal(items[0]?.item_id, "00000000-0000-4000-8000-000000000024");
  assert.equal(items[0]?.item_name, "Test product");
  assert.equal(items[0]?.item_variant, "Test variant");
  const metaCalls = fbqCalls(environment.window).filter(
    (entry) => entry[1] === "InitiateCheckout",
  );
  assert.equal(metaCalls.length, 2);
  assert.deepEqual(
    metaCalls.map((entry) => entry[3]),
    [{ eventID: eventId }, { eventID: eventId }],
  );
});

test("maps browser Meta standard event payloads", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  const viewContentId = environment.analytics.viewContent("product-1");
  const searchId = environment.analytics.search({ query: "shoes" });
  const calls = fbqCalls(environment.window);
  const findCall = (name: string) => calls.find((call) => call[1] === name);

  assert.equal(findCall("PageView"), undefined);
  assert.deepEqual(findCall("ViewContent")?.[2], {
    content_ids: ["product-1"],
    content_type: "product",
  });
  assert.deepEqual(findCall("Search")?.[2], { search_string: "shoes" });
  assert.deepEqual(findCall("ViewContent")?.[3], { eventID: viewContentId });
  assert.deepEqual(findCall("Search")?.[3], { eventID: searchId });
});

test("lets the Google tag collect PageView automatically", () => {
  const environment = harness({
    providers: { ga4: { measurementId: "G-TEST1234" } },
  });
  const config = ga4ConfigCalls(environment.window);
  assert.equal(config.length, 1);
  assert.equal(config[0]?.[1], "G-TEST1234");
  assert.equal(config[0]?.[2], undefined);
  assert.equal(
    ga4Calls(environment.window).some((call) => call[1] === "page_view"),
    false,
  );
});

test("maps purchase items to Meta content fields", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  const eventId = environment.analytics.recordPurchase({
    orderId: "00000000-0000-4000-8000-000000000999",
    valueMinor: 1_299,
    currency: "usd",
    items: [
      {
        productId: "product-1",
        productVariantId: "variant-1",
        quantity: 2,
        priceMinor: 649,
      },
    ],
  });
  const purchase = fbqCalls(environment.window).find(
    (call) => call[1] === "Purchase",
  );
  assert.equal(eventId, "00000000-0000-4000-8000-000000000999");
  assert.deepEqual(purchase?.[2], {
    content_ids: ["product-1"],
    content_type: "product",
    value: 12.99,
    currency: "USD",
    contents: [{ id: "product-1", quantity: 2, item_price: 6.49 }],
    num_items: 2,
    order_id: "00000000-0000-4000-8000-000000000999",
  });
});

test("uses the zero-decimal MGA currency scale in browser Meta payloads", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  environment.analytics.recordPurchase({
    orderId: "00000000-0000-4000-8000-000000000998",
    valueMinor: 1_299,
    currency: "mga",
    items: [
      {
        productId: "product-1",
        productVariantId: "variant-1",
        quantity: 1,
        priceMinor: 1_299,
      },
    ],
  });
  const purchase = fbqCalls(environment.window).find(
    (call) => call[1] === "Purchase",
  );
  assert.deepEqual(purchase?.[2], {
    content_ids: ["product-1"],
    content_type: "product",
    value: 1_299,
    currency: "MGA",
    contents: [{ id: "product-1", quantity: 1, item_price: 1_299 }],
    num_items: 1,
    order_id: "00000000-0000-4000-8000-000000000998",
  });
});

test("Purchase gives GA4 net item revenue and keeps Meta's paid total", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" }, ga4: { measurementId: "G-TEST1234" } },
  });
  const id = "00000000-0000-4000-8000-000000000997";
  environment.analytics.recordPurchase({
    orderId: id,
    currency: "USD",
    valueMinor: 2420,
    ga4ValueMinor: 2001,
    taxMinor: 119,
    shippingMinor: 300,
    items: [{
      productId: "product-1",
      productVariantId: "variant-1",
      itemName: "Test product",
      itemVariant: "Test variant",
      quantity: 2,
      priceMinor: 1100,
    }],
  });
  const pixel = fbqCalls(environment.window).find((call) => call[1] === "Purchase");
  assert.equal((pixel?.[2] as { value: number }).value, 24.20);
  assert.deepEqual(pixel?.[3], { eventID: id });
  const ga4 = ga4Calls(environment.window).find((call) => call[1] === "purchase")?.[2] as Record<string, unknown>;
  assert.equal(ga4.transaction_id, id);
  assert.equal(ga4.value, 20.01);
  assert.equal(ga4.tax, 1.19);
  assert.equal(ga4.shipping, 3);
  const items = ga4.items as Array<{
    item_name?: string;
    item_variant?: string;
    price: number;
    quantity: number;
    discount: number;
  }>;
  assert.equal(items[0]?.item_name, "Test product");
  assert.equal(items[0]?.item_variant, "Test variant");
  assert.equal(items.reduce((sum, item) => sum + item.price * item.quantity, 0).toFixed(2), "20.01");
  assert.equal(items.reduce((sum, item) => sum + item.discount * item.quantity, 0).toFixed(2), "1.99");
  environment.analytics.recordPurchase({
    orderId: id, currency: "USD", valueMinor: 2420, ga4ValueMinor: 2001,
    items: [{ productId: "product-1", productVariantId: "variant-1", quantity: 2, priceMinor: 1100 }],
  });
  const pixelPurchases = fbqCalls(environment.window).filter(
    (call) => call[1] === "Purchase",
  );
  const ga4Purchases = ga4Calls(environment.window).filter(
    (call) => call[1] === "purchase",
  );
  assert.equal(pixelPurchases.length, 2);
  assert.deepEqual(
    pixelPurchases.map((call) => call[3]),
    [{ eventID: id }, { eventID: id }],
  );
  assert.equal(ga4Purchases.length, 2);
  assert.deepEqual(
    ga4Purchases.map((call) => (call[2] as { transaction_id: string }).transaction_id),
    [id, id],
  );
});

test("a failed Pixel call can retry while stable platform ids handle repeats", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" }, ga4: { measurementId: "G-TEST1234" } },
  });
  const window = environment.window as unknown as { fbq: (...args: unknown[]) => void };
  const original = window.fbq;
  window.fbq = () => { throw new Error("Pixel unavailable"); };
  const input = {
    orderId: "00000000-0000-4000-8000-000000000996",
    valueMinor: 1000,
    currency: "USD",
    items: [{ productId: "product-1", productVariantId: "variant-1", quantity: 1, priceMinor: 1000 }],
  };
  environment.analytics.recordPurchase(input);
  window.fbq = original;
  environment.analytics.recordPurchase(input);
  assert.equal(fbqCalls(environment.window).filter((call) => call[1] === "Purchase").length, 1);
  assert.equal(ga4Calls(environment.window).filter((call) => call[1] === "purchase").length, 2);
});

test("manual Purchase remains compatible with the original minimal Order input", () => {
  const environment = harness({ providers: { ga4: { measurementId: "G-TEST1234" } } });
  environment.analytics.recordConfirmedPurchase({
    id: "00000000-0000-4000-8000-000000000995",
    status: "confirmed", payment_status: "paid", currency: "USD", total_amount_minor: 1000,
    lines: [{
      product_id: "product-1", product_variant_id: "variant-1", quantity: 1,
      unit_price_amount_minor: 1000, subtotal_amount_minor: 1000,
    } as never],
  });
  assert.equal(ga4Calls(environment.window).filter((call) => call[1] === "purchase").length, 1);
});

test("a recently refunded Order still represents the original Purchase", () => {
  const environment = harness({ providers: { ga4: { measurementId: "G-TEST1234" } } });
  environment.analytics.recordConfirmedPurchase({
    id: "00000000-0000-4000-8000-000000000994",
    status: "confirmed", payment_status: "refunded", currency: "USD", total_amount_minor: 1000,
    subtotal_amount_minor: 1000, discount_amount_minor: 0, tax_amount_minor: 0, shipping_amount_minor: 0,
    lines: [{
      product_id: "product-1", product_variant_id: "variant-1", quantity: 1,
      unit_price_amount_minor: 1000, subtotal_amount_minor: 1000,
    } as never],
  });
  assert.equal(ga4Calls(environment.window).filter((call) => call[1] === "purchase").length, 1);
});

test("setShopperId hashes the shopper id into Meta's external_id", async () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  environment.analytics.setShopperId(
    "01a0b983-9909-7990-a021-01afc9aab9c8",
  );
  const calls = await waitForAdvancedMatchingCalls(environment.window);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0]?.[1], "12345");
  assert.deepEqual(calls[0]?.[2], {
    // sha256("01a0b983-9909-7990-a021-01afc9aab9c8"), matching chaos-rust's
    // `sha256_hex(shopper_id.to_string())` in adapters/integrations/analytics/meta.rs
    external_id:
      "40a1c55e0d9ff00791dbd957c2505a2ba85be3e4af228393ae7ae06e7caa9dbd",
  });
});

test("setShopperId ignores a repeated call for the same shopper id", async () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  environment.analytics.setShopperId("01a0b983-9909-7990-a021-01afc9aab9c8");
  await waitForAdvancedMatchingCalls(environment.window);
  environment.analytics.setShopperId("01a0b983-9909-7990-a021-01afc9aab9c8");
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(fbqAdvancedMatchingCalls(environment.window).length, 1);
});

test("Meta matching is initialized at most once per page", async () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  environment.analytics.setShopperId("01a0b983-9909-7990-a021-01afc9aab9c8");
  await waitForAdvancedMatchingCalls(environment.window);
  environment.analytics.clearShopperId();
  environment.analytics.setShopperId("01a0b983-9909-7990-a021-01afc9aab9c9");
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(fbqAdvancedMatchingCalls(environment.window).length, 1);
});

test("does not use an anonymous shopper id as GA4 User-ID", () => {
  const environment = harness({
    providers: { ga4: { measurementId: "G-TEST1234" } },
  });
  environment.analytics.setShopperId("01a0b983-9909-7990-a021-01afc9aab9c8");
  assert.equal(
    (environment.window as unknown as { dataLayer: unknown[][] }).dataLayer.some(
      (call) => call[0] === "set",
    ),
    false,
  );
});
