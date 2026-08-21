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

  dispatch(name: string): void {
    for (const listener of this.listeners.get(name) ?? []) listener();
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

  removeItem(key: string): void {
    this.values.delete(key);
  }

  keys(): string[] {
    return [...this.values.keys()];
  }
}

function harness(
  responses: Array<{ ok: boolean; status: number }> = [{ ok: true, status: 200 }],
  initialConsent = false,
  options: {
    localStorage?: MemoryStorage;
    sessionStorage?: MemoryStorage;
    search?: string;
    referrer?: string;
    privacyMode?: "opt_in" | "opt_out";
    setInterval?: typeof setInterval;
    clearInterval?: typeof clearInterval;
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
  const requests: Array<{ url: string; options: { body: string } }> = [];
  const scripts: Array<{ id: string; src: string; async: boolean }> = [];
  const document = Object.assign(new FakeTarget(), {
    visibilityState: "visible",
    title: "Catalog",
    referrer: options.referrer ?? "https://search.example/results?q=private",
    location: {
      pathname: "/products",
      search:
        options.search ??
        "?utm_source=Newsletter&utm_medium=email&utm_campaign=Launch&utm_id=C1&utm_term=shoes&utm_content=hero&fbclid=fb-secret&gclid=g-secret&ignored=secret",
    },
    hasFocus: () => focused,
    getElementById: (id: string) => scripts.find((script) => script.id === id),
    createElement: () => ({ id: "", src: "", async: false }),
    head: { appendChild: (script: { id: string; src: string; async: boolean }) => scripts.push(script) },
  });
  const window = Object.assign(new FakeTarget(), {
    localStorage: options.localStorage ?? new MemoryStorage(),
    sessionStorage: options.sessionStorage ?? new MemoryStorage(),
    history: {
      pushState: (_data: unknown, _unused: string, _url?: string | URL | null) => {},
      replaceState: (_data: unknown, _unused: string, _url?: string | URL | null) => {},
    },
  });
  const analytics = createStorefrontAnalytics({
    publishableKey: "pk_test",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    document: document as any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    window: window as any,
    now: () => time,
    monotonicNow: () => elapsed,
    randomUUID: () => `00000000-0000-4000-8000-${String(++sequence).padStart(12, "0")}`,
    setInterval: options.setInterval ?? (() => 1) as unknown as typeof setInterval,
    clearInterval: options.clearInterval ?? (() => {}) as unknown as typeof clearInterval,
    privacyMode: options.privacyMode ?? "opt_in",
    ...(options.providers ? { providers: options.providers } : {}),
    fetch: (async (url: string, options: { body: string }) => {
      requests.push({ url, options });
      const response = responses.shift() ?? { ok: true, status: 200 };
      return { ...response, json: async () => ({ data: {} }) } as Response;
    }) as unknown as typeof fetch,
    ...(initialConsent
      ? { consent: { analyticsStorage: true, advertisingStorage: false, policyVersion: "cmp-v1" } }
      : {}),
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
    shiftWallClock: (milliseconds: number) => {
      time += milliseconds;
    },
    focus: (value: boolean) => {
      focused = value;
    },
  };
}

test("binds timer callbacks to the global receiver", async () => {
  function brandedTimer(this: unknown): number {
    assert.equal(this, globalThis);
    return 1;
  }

  const environment = harness([], false, {
    setInterval: brandedTimer as unknown as typeof setInterval,
    clearInterval: brandedTimer as unknown as typeof clearInterval,
  });
  environment.analytics.setConsent({ analyticsStorage: true, advertisingStorage: false, policyVersion: "cmp-v1" });
  environment.analytics.start();
  await environment.analytics.stop();
});

test("initial consent automatically starts page and SPA navigation tracking", async () => {
  const { analytics, window, requests } = harness([{ ok: true, status: 200 }], true);
  window.history.pushState({}, "", "/next");
  await analytics.flush();

  const events = JSON.parse(requests[0]!.options.body).events;
  assert.deepEqual(
    events.map((event: { event_name: string }) => event.event_name),
    ["page_view", "page_view"],
  );
  assert.match(window.localStorage.keys()[0]!, /^chaos\.analytics\.[a-z0-9]+\.visitor_id$/);
});

test("does not queue or transmit behavior without analytics storage consent", async () => {
  const { analytics, requests } = harness();
  assert.equal(analytics.pageView(), null);
  assert.equal(await analytics.flush(), null);
  assert.equal(requests.length, 0);
});

test("counts only visible and focused engagement and omits full referrer URLs", async () => {
  const environment = harness();
  const { analytics, document, window, requests, advance, focus } = environment;
  analytics.setConsent({ analyticsStorage: true, advertisingStorage: false, policyVersion: "cmp-v1" });
  analytics.start();
  const pageViewEventId = analytics.pageView();
  advance(15_000);
  focus(false);
  window.dispatch("blur");
  advance(30_000);
  document.visibilityState = "hidden";
  document.dispatch("visibilitychange");
  analytics.flushViewDuration();
  await analytics.flush();

  const events = JSON.parse(requests[0]!.options.body).events;
  assert.equal(events[0].event_name, "page_view");
  assert.equal(events[0].properties.referrer_domain, "search.example");
  assert.deepEqual(events[0].traffic.session, {
    source: "Newsletter",
    medium: "email",
    campaign: "Launch",
    campaign_id: "C1",
    term: "shoes",
    content: "hero",
    referrer_domain: "search.example",
  });
  assert.equal(events[0].traffic.session.fbclid, undefined);
  assert.equal(events[0].traffic.session.gclid, undefined);
  assert.equal(JSON.stringify(events).includes("q=private"), false);
  assert.equal(JSON.stringify(events).includes("ignored"), false);
  assert.equal(events[1].event_name, "view_duration");
  assert.equal(events[1].properties.page_view_event_id, pageViewEventId);
  assert.equal(events[1].properties.active_milliseconds, 15_000);
});

test("splits delayed active time into server-bounded heartbeat intervals", async () => {
  const { analytics, advance, requests } = harness();
  analytics.setConsent({ analyticsStorage: true, advertisingStorage: true, policyVersion: "cmp-v2" });
  analytics.start();
  analytics.pageView({ path: "/products/example" });
  advance(125_000);
  assert.equal(analytics.flushViewDuration(), 125_000);
  await analytics.flush();

  const heartbeats = JSON.parse(requests[0]!.options.body).events.slice(1);
  assert.deepEqual(
    heartbeats.map((event: { properties: { active_milliseconds: number } }) => event.properties.active_milliseconds),
    [60_000, 60_000, 5_000],
  );
});

test("flushes the previous page engagement before an SPA navigation", async () => {
  const { analytics, advance, requests } = harness();
  analytics.setConsent({ analyticsStorage: true, advertisingStorage: false, policyVersion: "cmp-v1" });
  analytics.start();
  const firstPage = analytics.pageView({ path: "/first" });
  advance(8_000);
  analytics.pageView({ path: "/second" });
  await analytics.flush();

  const events = JSON.parse(requests[0]!.options.body).events;
  assert.equal(events[1].event_name, "view_duration");
  assert.equal(events[1].properties.page_view_event_id, firstPage);
  assert.equal(events[1].properties.active_milliseconds, 8_000);
  assert.equal(events[2].properties.path, "/second");
});

test("requeues failed batches with stable event identities for server deduplication", async () => {
  const environment = harness([
    { ok: false, status: 503 },
    { ok: true, status: 200 },
  ]);
  const { analytics, requests } = environment;
  analytics.setConsent({ analyticsStorage: true, advertisingStorage: false, policyVersion: "cmp-v1" });
  const eventId = analytics.viewContent({ productId: "00000000-0000-4000-8000-000000000100" });
  await assert.rejects(analytics.flush(), /HTTP 503/);
  await analytics.flush();

  const first = JSON.parse(requests[0]!.options.body).events[0];
  const second = JSON.parse(requests[1]!.options.body).events[0];
  assert.equal(first.event_id, eventId);
  assert.deepEqual(second, first);
});

test("requeues failed batches in opt-out store-policy mode", async () => {
  const environment = harness([
    { ok: false, status: 503 },
    { ok: true, status: 200 },
  ], false, { privacyMode: "opt_out" });
  const { analytics, requests } = environment;
  analytics.pageView();
  await assert.rejects(analytics.flush(), /HTTP 503/);
  await analytics.flush();

  assert.equal(requests.length, 2);
});

test("consent revocation drops unsent events and future engagement", async () => {
  const { analytics, advance, requests } = harness();
  analytics.setConsent({ analyticsStorage: true, advertisingStorage: true, policyVersion: "cmp-v1" });
  analytics.start();
  analytics.pageView();
  advance(10_000);
  analytics.setConsent({ analyticsStorage: false, advertisingStorage: false, policyVersion: "cmp-v2" });
  analytics.flushViewDuration();
  await analytics.flush();
  assert.equal(requests.length, 0);
});

test("drains every queued batch and persists failed events for a later page load", async () => {
  const localStorage = new MemoryStorage();
  const sessionStorage = new MemoryStorage();
  const first = harness([{ ok: false, status: 503 }], false, { localStorage, sessionStorage });
  first.analytics.setConsent({ analyticsStorage: true, advertisingStorage: false, policyVersion: "cmp-v1" });
  for (let index = 0; index < 25; index += 1) {
    first.analytics.viewContent({ productId: "00000000-0000-4000-8000-000000000100" });
  }
  await assert.rejects(first.analytics.flush(), /HTTP 503/);

  const second = harness(
    [{ ok: true, status: 200 }, { ok: true, status: 200 }],
    true,
    { localStorage, sessionStorage },
  );
  await second.analytics.flush();
  assert.deepEqual(
    second.requests.map((request) => JSON.parse(request.options.body).events.length),
    [20, 6],
  );
});

test("uses a monotonic clock and resumes engagement after BFCache restoration", async () => {
  const { analytics, window, requests, advance, shiftWallClock } = harness();
  analytics.setConsent({ analyticsStorage: true, advertisingStorage: false, policyVersion: "cmp-v1" });
  analytics.start();
  analytics.pageView();
  advance(5_000);
  shiftWallClock(-86_400_000);
  window.dispatch("pagehide");
  advance(30_000);
  window.dispatch("pageshow");
  advance(7_000);
  analytics.flushViewDuration();
  await analytics.flush();

  const durations = requests
    .flatMap((request) => JSON.parse(request.options.body).events)
    .filter((event: { event_name: string }) => event.event_name === "view_duration")
    .map((event: { properties: { active_milliseconds: number } }) => event.properties.active_milliseconds);
  assert.deepEqual(durations, [5_000, 7_000]);
});

test("keeps first touch and updates last non-direct across browser sessions", async () => {
  const localStorage = new MemoryStorage();
  const first = harness([{ ok: true, status: 200 }], true, {
    localStorage,
    search: "?utm_source=meta&utm_campaign=launch&fbclid=allowed",
  });
  first.analytics.setConsent({ analyticsStorage: true, advertisingStorage: true, policyVersion: "cmp-v2" });
  first.analytics.pageView();
  await first.analytics.flush();

  const second = harness([{ ok: true, status: 200 }], true, {
    localStorage,
    sessionStorage: new MemoryStorage(),
    search: "?utm_source=google&utm_medium=cpc&gclid=google-click",
    referrer: "https://google.example/search?q=private",
  });
  second.analytics.setConsent({ analyticsStorage: true, advertisingStorage: true, policyVersion: "cmp-v2" });
  second.analytics.pageView();
  await second.analytics.flush();
  const events = second.requests.flatMap((request) => JSON.parse(request.options.body).events);
  const traffic = events.at(-1).traffic;
  assert.equal(traffic.first.source, "meta");
  assert.equal(traffic.session.source, "google");
  assert.equal(traffic.last_non_direct.source, "google");
  assert.equal(traffic.session.gclid, "google-click");
});

test("opt-out mode records and enables configured providers by default", async () => {
  const { analytics, requests, scripts } = harness([{ ok: true, status: 200 }], false, {
    privacyMode: "opt_out",
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  await analytics.flush();
  const event = JSON.parse(requests[0]!.options.body).events[0];
  assert.equal(event.event_name, "page_view");
  assert.equal(event.consent.analytics_storage, false);
  assert.equal(event.collection_basis, "store_policy");
  assert.deepEqual(
    scripts.map((script) => script.id).sort(),
    ["chaos-google-tag", "chaos-meta-pixel"],
  );
});

test("maps one stable event identity to Meta Pixel and GA4 in default opt-out mode", async () => {
  const { analytics, window, scripts } = harness([], false, {
    privacyMode: "opt_out",
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  const eventId = analytics.addToCart({
    cartId: "00000000-0000-4000-8000-000000000200",
    productVariantId: "00000000-0000-4000-8000-000000000100",
    quantity: 2,
  });

  assert.deepEqual(
    scripts.map((script) => script.id).sort(),
    ["chaos-google-tag", "chaos-meta-pixel"],
  );
  const metaTrack = (window as unknown as { fbq: { queue: unknown[][] } }).fbq.queue.find(
    (call) => call[0] === "track" && call[1] === "AddToCart",
  );
  assert.equal(metaTrack?.[1], "AddToCart");
  assert.deepEqual(metaTrack?.[3], { eventID: eventId });
  const gaTrack = (window as unknown as { dataLayer: unknown[][] }).dataLayer.find(
    (call) => call[0] === "event" && call[1] === "add_to_cart",
  );
  assert.equal((gaTrack?.[2] as { event_id: string }).event_id, eventId);
});

test("projects a confirmed Purchase once with the Order ID shared by Pixel and GA4", () => {
  const storage = new MemoryStorage();
  const { analytics, window } = harness([], false, {
    localStorage: storage,
    privacyMode: "opt_out",
    providers: {
      metaPixel: { pixelId: "12345" },
      ga4: { measurementId: "G-TEST1234" },
    },
  });
  const orderId = "00000000-0000-4000-8000-000000000900";
  const input = {
    orderId,
    valueMinor: 12_345,
    currency: "usd",
    items: [{ itemId: "variant-1", quantity: 2, priceMinor: 6_172 }],
  };

  const paymentAttemptId = "00000000-0000-4000-8000-000000000800";
  assert.equal(
    analytics.addPaymentInfo({
      paymentAttemptId,
      orderId,
      valueMinor: 12_345,
      currency: "USD",
    }),
    paymentAttemptId,
  );
  assert.equal(
    analytics.addPaymentInfo({ paymentAttemptId, orderId, valueMinor: 12_345, currency: "USD" }),
    null,
  );
  assert.equal(analytics.purchase(input), orderId);
  assert.equal(analytics.purchase(input), null);
  const metaPurchases = (window as unknown as { fbq: { queue: unknown[][] } }).fbq.queue.filter(
    (call) => call[0] === "track" && call[1] === "Purchase",
  );
  assert.equal(metaPurchases.length, 1);
  assert.deepEqual(metaPurchases[0]?.[3], { eventID: orderId });
  assert.equal((metaPurchases[0]?.[2] as { value: number }).value, 123.45);
  const metaPaymentInfo = (window as unknown as { fbq: { queue: unknown[][] } }).fbq.queue.filter(
    (call) => call[0] === "track" && call[1] === "AddPaymentInfo",
  );
  assert.equal(metaPaymentInfo.length, 1);
  assert.deepEqual(metaPaymentInfo[0]?.[3], { eventID: paymentAttemptId });
  const gaPurchases = (window as unknown as { dataLayer: unknown[][] }).dataLayer.filter(
    (call) => call[0] === "event" && call[1] === "purchase",
  );
  assert.equal(gaPurchases.length, 1);
  assert.equal((gaPurchases[0]?.[2] as { transaction_id: string }).transaction_id, orderId);
  const gaPaymentInfo = (window as unknown as { dataLayer: unknown[][] }).dataLayer.filter(
    (call) => call[0] === "event" && call[1] === "add_payment_info",
  );
  assert.equal(gaPaymentInfo.length, 1);
});

test("an explicit opt-out stops first-party and provider collection", async () => {
  const { analytics, requests, window } = harness([], false, {
    privacyMode: "opt_out",
    providers: { metaPixel: { pixelId: "12345" }, ga4: { measurementId: "G-TEST1234" } },
  });
  await analytics.flush();
  const metaCallsBefore = (window as unknown as { fbq: { queue: unknown[][] } }).fbq.queue.length;
  analytics.setConsent({
    analyticsStorage: false,
    advertisingStorage: false,
    policyVersion: "user-opt-out-v1",
  });
  assert.equal(analytics.pageView(), null);
  assert.equal(await analytics.flush(), null);
  assert.equal(requests.length, 1);
  const metaCalls = (window as unknown as { fbq: { queue: unknown[][] } }).fbq.queue;
  assert.equal(metaCalls.length, metaCallsBefore + 1);
  assert.deepEqual(metaCalls.at(-1), ["consent", "revoke"]);
});
