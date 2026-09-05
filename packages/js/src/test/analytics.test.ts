import assert from "node:assert/strict";
import test from "node:test";

import { createStorefrontAnalytics } from "../analytics.js";

class FakeTarget {
  private readonly listeners = new Map<string, Set<() => void>>();

  addEventListener(name: string, listener: () => void): void {
    const listeners = this.listeners.get(name) ?? new Set();
    listeners.add(listener);
    this.listeners.set(name, listeners);
  }

  removeEventListener(name: string, listener: () => void): void {
    this.listeners.get(name)?.delete(listener);
  }
}

class MemoryStorage {
  private readonly values = new Map<string, string>();

  get length(): number {
    return this.values.size;
  }

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }

  key(index: number): string | null {
    return [...this.values.keys()][index] ?? null;
  }
}

function harness(
  options: {
    autoStart?: boolean;
    localStorage?: MemoryStorage;
    sessionStorage?: MemoryStorage;
    search?: string;
    referrer?: string;
    cookie?: string;
    href?: string;
    userAgent?: string;
    providers?: {
      metaPixel?: { pixelId: string };
      ga4?: { measurementId: string };
    };
  } = {},
) {
  let time = Date.parse("2026-08-16T00:00:00Z");
  let sequence = 0;
  const scripts: Array<{ id: string; src: string; async: boolean }> = [];
  const document = Object.assign(new FakeTarget(), {
    cookie: options.cookie ?? "",
    visibilityState: "visible",
    title: "Catalog",
    referrer: options.referrer ?? "https://search.example/results?q=private",
    location: {
      href: options.href ?? "https://shop.example/products",
      pathname: "/products",
      search: options.search ?? "?fbclid=fb-secret&gclid=g-secret",
    },
    getElementById: (id: string) => scripts.find((script) => script.id === id),
    createElement: () => ({ id: "", src: "", async: false }),
    head: {
      appendChild: (script: { id: string; src: string; async: boolean }) =>
        scripts.push(script),
    },
  });
  const window = Object.assign(new FakeTarget(), {
    localStorage: options.localStorage ?? new MemoryStorage(),
    sessionStorage: options.sessionStorage ?? new MemoryStorage(),
    history: {
      pushState: (
        _data: unknown,
        _unused: string,
        _url?: string | URL | null,
      ) => {},
      replaceState: (
        _data: unknown,
        _unused: string,
        _url?: string | URL | null,
      ) => {},
    },
    navigator: { userAgent: options.userAgent ?? "ChaosTest/1.0" },
  });
  const analytics = createStorefrontAnalytics({
    publishableKey: "public_test",
    document: document as unknown as Document,
    window: window as unknown as Window & typeof globalThis,
    now: () => time,
    randomUUID: () =>
      `00000000-0000-4000-8000-${String(++sequence).padStart(12, "0")}`,
    autoStart: options.autoStart ?? false,
    ...(options.providers ? { providers: options.providers } : {}),
  });
  return { analytics, document, window, scripts };
}

function ga4Calls(window: unknown): unknown[][] {
  return (window as { dataLayer: unknown[][] }).dataLayer.filter(
    (call) => call[0] === "event",
  );
}

function fbqCalls(window: unknown): unknown[][] {
  return (window as { fbq: { queue: unknown[][] } }).fbq.queue.filter(
    (call) => call[0] === "track",
  );
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

test("keeps history observation active when one of multiple analytics clients stops", () => {
  const first = harness({
    autoStart: false,
    providers: { ga4: { measurementId: "G-TEST1234" } },
  });
  const second = createStorefrontAnalytics({
    publishableKey: "public_test",
    document: first.document as unknown as Document,
    window: first.window as unknown as Window & typeof globalThis,
    now: () => 0,
    randomUUID: () => "00000000-0000-4000-8000-000000000099",
    autoStart: false,
    providers: { ga4: { measurementId: "G-TEST1234" } },
  });
  first.analytics.start();
  second.start();

  const pageViews = () =>
    ga4Calls(first.window).filter((call) => call[1] === "page_view").length;

  first.window.history.pushState({}, "", "/first");
  assert.equal(pageViews(), 2);

  first.analytics.stop();
  first.window.history.pushState({}, "", "/second");

  assert.equal(pageViews(), 3);
  second.stop();
});

test("keeps one stable provider event identity", () => {
  const environment = harness({
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  const eventId = environment.analytics.viewContent({
    productId: "00000000-0000-4000-8000-000000000200",
    productVariantId: "00000000-0000-4000-8000-000000000100",
  });
  const metaTrack = fbqCalls(environment.window).find(
    (call) => call[1] === "ViewContent",
  );
  assert.deepEqual(metaTrack?.[3], { eventID: eventId });
});

test("recordAddToCart dedupes a repeated explicit event ID", () => {
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

  // Re-sending the same event ID is a no-op: Meta dedup relies on exactly
  // one projection per ID.
  const secondId = environment.analytics.recordAddToCart(input, explicitEventId);
  assert.equal(secondId, null);
  assert.equal(
    fbqCalls(environment.window).filter((call) => call[1] === "AddToCart").length,
    1,
  );
});

test("prunes provider_event dedup keys older than 90 days on construction", () => {
  const localStorage = new MemoryStorage();
  const first = harness({ localStorage });
  const eventId = first.analytics.recordAddToCart({
    cartId: "00000000-0000-4000-8000-000000000001",
    productId: "00000000-0000-4000-8000-000000000002",
    productVariantId: "00000000-0000-4000-8000-000000000003",
    quantity: 1,
    priceMinor: 1_000,
    valueMinor: 1_000,
    currency: "usd",
  });
  let staleKey: string | null = null;
  for (let index = 0; index < localStorage.length; index += 1) {
    const key = localStorage.key(index);
    if (key?.includes(`add_to_cart.${eventId}`)) staleKey = key;
  }
  assert.ok(staleKey, "dedup key should exist after recording");
  const wellPastRetention = new Date(
    Date.parse("2026-08-16T00:00:00Z") - 91 * 24 * 60 * 60 * 1000,
  ).toISOString();
  localStorage.setItem(staleKey!, wellPastRetention);

  // Constructing a fresh instance against the same storage prunes on startup.
  harness({ localStorage });

  assert.equal(localStorage.getItem(staleKey!), null);
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
    content_ids: ["00000000-0000-4000-8000-000000000003"],
    content_type: "product",
    quantity: 2,
    value: 12.98,
    currency: "USD",
    contents: [
      { id: "00000000-0000-4000-8000-000000000003", quantity: 2, item_price: 6.49 },
    ],
    num_items: 2,
  });
});

test("recordCartMutation reuses a server-supplied event ID", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  const suppliedEventId = "00000000-0000-4000-8000-0000000000aa";
  const returnedId = environment.analytics.recordCartMutation({
    cart: {
      id: "00000000-0000-4000-8000-000000000030",
      currency: "USD",
      status: "active",
      version: 1,
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
    event_id: suppliedEventId,
  });
  assert.equal(returnedId, suppliedEventId);
  const metaTrack = fbqCalls(environment.window).find(
    (call) => call[1] === "AddToCart",
  );
  assert.deepEqual(metaTrack?.[3], { eventID: suppliedEventId });
});

test("records InitiateCheckout with the public order number", () => {
  const environment = harness({
    providers: { ga4: { measurementId: "G-TEST1234" } },
  });
  const eventId = environment.analytics.recordInitiateCheckout({
    cartId: "00000000-0000-4000-8000-000000000011",
    orderNumber: "W-20260830-7K4M9Q2D",
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
  assert.equal(parameters.transaction_id, "W-20260830-7K4M9Q2D");
  assert.equal(parameters.event_id, eventId);
});

test("attributes server checkout creation to the source Cart", () => {
  const environment = harness({
    providers: { ga4: { measurementId: "G-TEST1234" } },
  });
  const eventId = environment.analytics.recordCheckoutCreation({
    checkout: {
      order_number: "W-20260830-7K4M9Q2D",
      client_action: {
        type: "mount_embedded_checkout",
        public_key: "pk_test_stripe",
        client_token: "cs_test_secret",
      },
    },
    source_cart: {
      id: "00000000-0000-4000-8000-000000000021",
      currency: "USD",
      status: "locked",
      version: 1,
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
      version: 1,
      lines: [],
      subtotal_amount_minor: 0,
      created_at: "2026-08-16T00:00:00Z",
      updated_at: "2026-08-16T00:00:00Z",
    },
  });
  const call = ga4Calls(environment.window).find(
    (entry) => entry[1] === "begin_checkout",
  );
  const parameters = call?.[2] as Record<string, unknown>;
  assert.equal(parameters.transaction_id, "W-20260830-7K4M9Q2D");
  assert.equal(parameters.event_id, eventId);
  const items = parameters.items as Array<Record<string, unknown>>;
  assert.equal(items[0]?.item_id, "00000000-0000-4000-8000-000000000025");
});

test("maps browser Meta standard event payloads", () => {
  const environment = harness({
    providers: { metaPixel: { pixelId: "12345" } },
  });
  const pageViewId = environment.analytics.pageView({
    path: "/products",
    title: "Shoes",
  });
  const viewContentId = environment.analytics.viewContent({
    productId: "product-1",
    productVariantId: "variant-1",
  });
  const searchId = environment.analytics.search({ query: "shoes" });
  const calls = fbqCalls(environment.window);
  const findCall = (name: string) => calls.find((call) => call[1] === name);

  assert.deepEqual(findCall("PageView")?.[2], { page_path: "/products" });
  assert.deepEqual(findCall("ViewContent")?.[2], {
    content_ids: ["variant-1"],
    content_type: "product",
  });
  assert.deepEqual(findCall("Search")?.[2], { search_string: "shoes" });
  assert.deepEqual(findCall("PageView")?.[3], { eventID: pageViewId });
  assert.deepEqual(findCall("ViewContent")?.[3], { eventID: viewContentId });
  assert.deepEqual(findCall("Search")?.[3], { eventID: searchId });
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
    content_ids: ["variant-1"],
    content_type: "product",
    value: 12.99,
    currency: "USD",
    contents: [{ id: "variant-1", quantity: 2, item_price: 6.49 }],
    num_items: 2,
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
    content_ids: ["variant-1"],
    content_type: "product",
    value: 1_299,
    currency: "MGA",
    contents: [{ id: "variant-1", quantity: 1, item_price: 1_299 }],
    num_items: 1,
  });
});
