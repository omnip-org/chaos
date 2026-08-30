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

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
}

function harness(
  responses: Array<{ ok: boolean; status: number; body?: unknown }> = [
    { ok: true, status: 200 },
  ],
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
  let elapsed = 0;
  let sequence = 0;
  let focused = true;
  const requests: Array<{
    url: string;
    options: { body: string; headers?: Record<string, string> };
  }> = [];
  const scripts: Array<{ id: string; src: string; async: boolean }> = [];
  const document = Object.assign(new FakeTarget(), {
    cookie: options.cookie ?? "",
    visibilityState: "visible",
    title: "Catalog",
    referrer: options.referrer ?? "https://search.example/results?q=private",
    location: {
      href: options.href ?? "https://shop.example/products",
      pathname: "/products",
      search:
        options.search ??
        "?utm_source=Newsletter&utm_medium=email&utm_campaign=Launch&utm_id=C1&utm_term=shoes&utm_content=hero&fbclid=fb-secret&gclid=g-secret&ignored=secret",
    },
    hasFocus: () => focused,
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
    getShopperToken: () => "shopper-token",
    document: document as unknown as Document,
    window: window as unknown as Window & typeof globalThis,
    now: () => time,
    monotonicNow: () => elapsed,
    randomUUID: () =>
      `00000000-0000-4000-8000-${String(++sequence).padStart(12, "0")}`,
    setInterval: (() => 1) as unknown as typeof setInterval,
    clearInterval: (() => {}) as unknown as typeof clearInterval,
    autoStart: options.autoStart ?? false,
    ...(options.providers ? { providers: options.providers } : {}),
    fetch: (async (
      url: string,
      request: { body: string; headers?: Record<string, string> },
    ) => {
      requests.push({ url, options: request });
      const response = responses.shift() ?? { ok: true, status: 200 };
      return {
        ...response,
        json: async () => response.body ?? { data: {} },
      } as Response;
    }) as unknown as typeof fetch,
  });
  return {
    analytics,
    document,
    window,
    requests,
    scripts,
    advance: (milliseconds: number) => {
      time += milliseconds;
      elapsed += milliseconds;
    },
    focus: (value: boolean) => {
      focused = value;
    },
  };
}

test("starts collection and records valid custom behavior names", async () => {
  const environment = harness([{ ok: true, status: 200 }], { autoStart: true });
  const eventId = environment.analytics.track("store_defined_event", {
    product_id: "product-1",
  });
  await environment.analytics.flush();
  const events = environment.requests.flatMap(
    (request) => JSON.parse(request.options.body).events,
  );
  assert.equal(events[0].event_name, "page_view");
  assert.equal(events[1].event_id, eventId);
  assert.equal(events[1].properties.product_id, "product-1");
  assert.equal(typeof events[1].properties.session_id, "string");
  assert.equal("consent" in events[1], false);
  assert.equal("schema_version" in events[1], false);
});

test("retries a generic event with the same event ID", async () => {
  const environment = harness([
    { ok: false, status: 503 },
    { ok: true, status: 200 },
  ]);
  const eventId = environment.analytics.track("coupon_applied", {
    code: "WELCOME",
  });
  await assert.rejects(environment.analytics.flush(), /HTTP 503/);
  await environment.analytics.flush();
  const first = JSON.parse(environment.requests[0]!.options.body).events[0];
  const second = JSON.parse(environment.requests[1]!.options.body).events[0];
  assert.equal(eventId, first.event_id);
  assert.equal(first.event_id, second.event_id);
});

test("stores traffic context inside dynamic properties", async () => {
  const environment = harness([{ ok: true, status: 200 }]);
  environment.analytics.pageView();
  await environment.analytics.flush();
  const event = JSON.parse(environment.requests[0]!.options.body).events[0];
  assert.equal(event.properties.traffic.first.source, "Newsletter");
  assert.equal(event.properties.traffic.session.source, "Newsletter");
  assert.equal(event.properties.traffic.session.fbclid, "fb-secret");
  assert.equal(event.properties.path, "/products");
});

test("stores the page URL without its fragment and Meta browser matching context", async () => {
  const environment = harness([{ ok: true, status: 200 }], {
    cookie:
      "_fbp=fb.1.1234567890123.browser; _fbc=fb.1.1234567890123.cookie-click",
    href: "https://shop.example/products?variant=1#token=secret-capability",
    search: "?variant=1",
    userAgent: "ChaosBrowser/2.0",
  });
  environment.analytics.pageView();
  await environment.analytics.flush();
  const event = JSON.parse(environment.requests[0]!.options.body).events[0];
  assert.equal(
    event.properties._meta.source_url,
    "https://shop.example/products?variant=1",
  );
  assert.equal(event.properties._meta.fbc, "fb.1.1234567890123.cookie-click");
  assert.equal(event.properties._meta.fbp, "fb.1.1234567890123.browser");
  assert.equal(event.properties._meta.client_user_agent, "ChaosBrowser/2.0");
});

test("prefers a current fbclid over a stale _fbc cookie and writes milliseconds", async () => {
  const environment = harness([{ ok: true, status: 200 }], {
    cookie: "_fbc=fb.1.1.old-click",
    search: "?fbclid=current-click",
  });
  environment.analytics.pageView();
  await environment.analytics.flush();
  const event = JSON.parse(environment.requests[0]!.options.body).events[0];
  const expectedFbc = `fb.1.${Date.parse("2026-08-16T00:00:00Z")}.current-click`;
  assert.equal(event.properties._meta.fbc, expectedFbc);
  assert.match(
    environment.document.cookie,
    new RegExp(`_fbc=${encodeURIComponent(expectedFbc)}`),
  );
});

test("retains a long Meta click identifier within the attribution bound", () => {
  const fbclid = "x".repeat(2_000);
  const environment = harness([], { search: `?fbclid=${fbclid}` });
  const event = environment.analytics.prepareCommerceEvent("add_to_cart");
  const meta = event.properties._meta as Record<string, unknown>;

  assert.equal(
    meta.fbc,
    `fb.1.${Date.parse("2026-08-16T00:00:00Z")}.${fbclid}`,
  );
});

test("does not lose valid events when a server rejects one event in a batch", async () => {
  const environment = harness([
    {
      ok: false,
      status: 422,
      body: {
        error: { code: "validation_failed", message: "invalid event name" },
      },
    },
    { ok: true, status: 200 },
    {
      ok: false,
      status: 422,
      body: {
        error: { code: "validation_failed", message: "invalid event name" },
      },
    },
  ]);
  const validEventId = environment.analytics.pageView();
  const queue = (environment.analytics as unknown as { queue: unknown[] })
    .queue;
  queue.push({
    event_id: "00000000-0000-4000-8000-000000000998",
    event_name: "Invalid-Name",
    occurred_at: "2026-08-16T00:00:00.000Z",
    properties: {},
  });

  await environment.analytics.flush();

  assert.equal(environment.requests.length, 3);
  assert.deepEqual(
    JSON.parse(environment.requests[1]!.options.body).events.map(
      (event: { event_id: string }) => event.event_id,
    ),
    [validEventId],
  );
  assert.equal(
    environment.analytics.track("valid_custom_event"),
    "00000000-0000-4000-8000-000000000004",
  );
});

test("rejects event names that the Storefront API cannot store", () => {
  const environment = harness();
  assert.throws(
    () => environment.analytics.track("Invalid-Name"),
    /eventName must be 1-64 lowercase snake_case characters/,
  );
});

test("sends generic events to GA4 once and requires the commerce envelope", () => {
  const environment = harness([], {
    providers: { ga4: { measurementId: "G-TEST1234" } },
  });
  environment.analytics.track("store_defined_event", {
    product_id: "product-1",
  });
  assert.throws(
    () => environment.analytics.track("purchase", { order_id: "order-1" }),
    /after the commerce operation succeeds/,
  );

  const events = (
    environment.window as unknown as { dataLayer: unknown[][] }
  ).dataLayer.filter((call) => call[0] === "event");
  assert.equal(events.length, 1);
  assert.equal(events[0]?.[1], "store_defined_event");
});

test("keeps history observation active when one of multiple analytics clients stops", async () => {
  const first = harness([], { autoStart: false });
  const second = createStorefrontAnalytics({
    publishableKey: "public_test",
    getShopperToken: () => "shopper-token",
    document: first.document as unknown as Document,
    window: first.window as unknown as Window & typeof globalThis,
    now: () => 0,
    monotonicNow: () => 0,
    randomUUID: () => "00000000-0000-4000-8000-000000000099",
    setInterval: (() => 1) as unknown as typeof setInterval,
    clearInterval: (() => {}) as unknown as typeof clearInterval,
    autoStart: false,
    fetch: (async () => ({
      ok: true,
      status: 200,
      json: async () => ({ data: { received: 1, stored: 1, duplicates: 0 } }),
    })) as unknown as typeof fetch,
  });
  first.analytics.start();
  second.start();

  first.window.history.pushState({}, "", "/first");
  const firstQueue = (first.analytics as unknown as { queue: unknown[] }).queue;
  const secondQueue = (second as unknown as { queue: unknown[] }).queue;
  assert.equal(firstQueue.length, 1);
  assert.equal(secondQueue.length, 1);

  await first.analytics.stop();
  first.window.history.pushState({}, "", "/second");

  assert.equal(secondQueue.length, 2);
  await second.stop();
});

test("does not turn a historical last-touch fbclid into a new fbc", async () => {
  const localStorage = new MemoryStorage();
  const first = harness([{ ok: true, status: 200 }], { localStorage });
  first.analytics.pageView();
  await first.analytics.flush();

  const later = harness([{ ok: true, status: 200 }], {
    localStorage,
    search: "",
  });
  later.analytics.pageView();
  await later.analytics.flush();
  const event = JSON.parse(later.requests[0]!.options.body).events[0];
  assert.equal(event.properties._meta.fbc, undefined);
});

test("splits active engagement into bounded behavior events", async () => {
  const environment = harness([{ ok: true, status: 200 }]);
  environment.analytics.pageView();
  environment.advance(125_000);
  assert.equal(environment.analytics.flushViewDuration(), 125_000);
  await environment.analytics.flush();
  const events = JSON.parse(environment.requests[0]!.options.body).events;
  assert.deepEqual(
    events
      .filter(
        (event: { event_name: string }) => event.event_name === "view_duration",
      )
      .map(
        (event: { properties: { active_milliseconds: number } }) =>
          event.properties.active_milliseconds,
      ),
    [60_000, 60_000, 5_000],
  );
});

test("keeps one stable provider event identity", () => {
  const environment = harness([], {
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  const eventId = environment.analytics.viewContent({
    productId: "00000000-0000-4000-8000-000000000200",
    productVariantId: "00000000-0000-4000-8000-000000000100",
  });
  const metaTrack = (
    environment.window as unknown as { fbq: { queue: unknown[][] } }
  ).fbq.queue.find((call) => call[0] === "track" && call[1] === "ViewContent");
  assert.deepEqual(metaTrack?.[3], { eventID: eventId });
});

test("does not send duration or arbitrary events to Meta Pixel", () => {
  const environment = harness([], {
    providers: { metaPixel: { pixelId: "12345" } },
  });
  environment.analytics.track("view_duration", { active_milliseconds: 1_000 });
  assert.throws(
    () =>
      environment.analytics.track("add_to_cart", { product_id: "product-1" }),
    /after the commerce operation succeeds/,
  );
  assert.throws(
    () =>
      environment.analytics.track("initiate_checkout", { order_id: "order-1" }),
    /after the commerce operation succeeds/,
  );
  environment.analytics.track("store_defined_event", {
    product_id: "product-1",
  });
  const trackCalls = (
    environment.window as unknown as { fbq: { queue: unknown[][] } }
  ).fbq.queue.filter((call) => call[0] === "track");
  assert.equal(trackCalls.length, 0);
});

test("records commerce attribution through the common endpoint after success", async () => {
  const environment = harness([], {
    cookie: "_fbp=fb.1.1234567890123.browser; _fbc=fb.1.1234567890123.click",
    search: "",
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  const event = environment.analytics.prepareCommerceEvent("add_to_cart", {
    product_id: "product-1",
    product_variant_id: "variant-1",
    quantity: 1,
  });

  assert.equal(
    (environment.analytics as unknown as { queue: unknown[] }).queue.length,
    0,
  );
  const meta = event.properties._meta as Record<string, unknown>;
  assert.equal(meta.fbc, "fb.1.1234567890123.click");
  assert.equal(meta.fbp, "fb.1.1234567890123.browser");
  assert.equal(typeof event.properties.session_id, "string");

  const projected = environment.analytics.sendCommerceEvent(event, {
    product_id: "product-1",
    product_variant_id: "variant-1",
    value_minor: 1_000,
    currency: "USD",
    items: [
      {
        product_id: "product-1",
        product_variant_id: "variant-1",
        quantity: 1,
        price_minor: 1_000,
      },
    ],
  });
  assert.equal(projected, event.event_id);
  const metaTrack = (
    environment.window as unknown as { fbq: { queue: unknown[][] } }
  ).fbq.queue.find((call) => call[0] === "track" && call[1] === "AddToCart");
  assert.deepEqual(metaTrack?.[3], { eventID: event.event_id });
  await environment.analytics.flush();
  const recorded = environment.requests.flatMap(
    (request) => JSON.parse(request.options.body).events,
  );
  assert.equal(recorded.length, 1);
  assert.equal(recorded[0].event_id, event.event_id);
  assert.equal(recorded[0].event_name, "add_to_cart");
  assert.equal(recorded[0].properties._meta.fbc, meta.fbc);
  assert.equal(recorded[0].properties._meta.fbp, meta.fbp);
  assert.equal(recorded[0].properties.value_minor, 1_000);
  assert.equal(
    (environment.analytics as unknown as { queue: unknown[] }).queue.length,
    0,
  );
});

test("high-level commerce methods own canonical event properties", async () => {
  const environment = harness([], {
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
  await environment.analytics.flush();

  const event = JSON.parse(environment.requests[0]!.options.body).events[0];
  assert.equal(event.event_id, eventId);
  assert.equal(event.event_name, "add_to_cart");
  assert.equal(
    event.properties.cart_id,
    "00000000-0000-4000-8000-000000000001",
  );
  assert.equal(
    event.properties.product_id,
    "00000000-0000-4000-8000-000000000002",
  );
  assert.equal(
    event.properties.product_variant_id,
    "00000000-0000-4000-8000-000000000003",
  );
  assert.equal(event.properties.quantity, 2);
  assert.equal(event.properties.value_minor, 1_298);
  assert.equal(event.properties.currency, "USD");
  assert.deepEqual(event.properties.items, [
    {
      product_id: "00000000-0000-4000-8000-000000000002",
      product_variant_id: "00000000-0000-4000-8000-000000000003",
      quantity: 2,
      price_minor: 649,
    },
  ]);
  assert.equal(typeof event.properties._meta, "object");
  assert.equal(typeof event.properties.session_id, "string");
  const metaTrack = (
    environment.window as unknown as { fbq: { queue: unknown[][] } }
  ).fbq.queue.find((call) => call[0] === "track" && call[1] === "AddToCart");
  assert.deepEqual(metaTrack?.[3], { eventID: eventId });
});

test("records InitiateCheckout with the public order number", async () => {
  const environment = harness([]);
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
  await environment.analytics.flush();

  const event = JSON.parse(environment.requests[0]!.options.body).events[0];
  assert.equal(event.event_id, eventId);
  assert.equal(event.properties.order_number, "W-20260830-7K4M9Q2D");
  assert.equal(event.properties.order_id, undefined);
});

test("attributes server checkout creation to the source Cart", async () => {
  const environment = harness([]);
  const eventId = environment.analytics.recordCheckoutCreation({
    checkout: {
      order_number: "W-20260830-7K4M9Q2D",
      source_cart_id: "00000000-0000-4000-8000-000000000021",
      client_action: {
        type: "mount_embedded_checkout",
        public_key: "pk_test_stripe",
        client_token: "cs_test_secret",
      },
    },
    source_cart: {
      id: "00000000-0000-4000-8000-000000000021",
      price_list_id: "00000000-0000-4000-8000-000000000023",
      currency: "USD",
      status: "locked",
      version: 1,
      lines: [
        {
          product_id: "00000000-0000-4000-8000-000000000024",
          product_variant_id: "00000000-0000-4000-8000-000000000025",
          product_title: "Test product",
          variant_title: "Test variant",
          track_inventory: false,
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
      price_list_id: "00000000-0000-0000-0000-000000000023",
      currency: "USD",
      status: "active",
      version: 1,
      lines: [],
      subtotal_amount_minor: 0,
      created_at: "2026-08-16T00:00:00Z",
      updated_at: "2026-08-16T00:00:00Z",
    },
  });
  await environment.analytics.flush();

  const event = JSON.parse(environment.requests[0]!.options.body).events[0];
  assert.equal(event.event_id, eventId);
  assert.equal(event.properties.cart_id, "00000000-0000-4000-8000-000000000021");
  assert.equal(event.properties.order_number, "W-20260830-7K4M9Q2D");
  assert.equal(event.properties.value_minor, 2_000);
  assert.equal(
    event.properties.items[0].product_variant_id,
    "00000000-0000-4000-8000-000000000025",
  );
});

test("maps browser Meta standard event payloads", () => {
  const environment = harness([], {
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
  const calls = (
    environment.window as unknown as { fbq: { queue: unknown[][] } }
  ).fbq.queue;
  const findCall = (name: string) =>
    calls.find((call) => call[0] === "track" && call[1] === name);

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
  const environment = harness([], {
    providers: { metaPixel: { pixelId: "12345" } },
  });
  const eventId = environment.analytics.purchase({
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
  const purchase = (
    environment.window as unknown as { fbq: { queue: unknown[][] } }
  ).fbq.queue.find((call) => call[0] === "track" && call[1] === "Purchase");
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
  const environment = harness([], {
    providers: { metaPixel: { pixelId: "12345" } },
  });
  environment.analytics.purchase({
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
  const purchase = (
    environment.window as unknown as { fbq: { queue: unknown[][] } }
  ).fbq.queue.find((call) => call[0] === "track" && call[1] === "Purchase");
  assert.deepEqual(purchase?.[2], {
    content_ids: ["variant-1"],
    content_type: "product",
    value: 1_299,
    currency: "MGA",
    contents: [{ id: "variant-1", quantity: 1, item_price: 1_299 }],
    num_items: 1,
  });
});
