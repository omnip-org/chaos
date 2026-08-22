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
  responses: Array<{ ok: boolean; status: number }> = [{ ok: true, status: 200 }],
  options: {
    autoStart?: boolean;
    localStorage?: MemoryStorage;
    sessionStorage?: MemoryStorage;
    search?: string;
    referrer?: string;
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
  const requests: Array<{ url: string; options: { body: string; headers?: Record<string, string> } }> = [];
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
    getShopperToken: () => "shopper-token",
    document: document as unknown as Document,
    window: window as unknown as Window & typeof globalThis,
    now: () => time,
    monotonicNow: () => elapsed,
    randomUUID: () => `00000000-0000-4000-8000-${String(++sequence).padStart(12, "0")}`,
    setInterval: (() => 1) as unknown as typeof setInterval,
    clearInterval: (() => {}) as unknown as typeof clearInterval,
    autoStart: options.autoStart ?? false,
    ...(options.providers ? { providers: options.providers } : {}),
    fetch: (async (url: string, request: { body: string; headers?: Record<string, string> }) => {
      requests.push({ url, options: request });
      const response = responses.shift() ?? { ok: true, status: 200 };
      return { ...response, json: async () => ({ data: {} }) } as Response;
    }) as unknown as typeof fetch,
  });
  return {
    analytics,
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

test("starts collection without a policy and accepts arbitrary behavior names", async () => {
  const environment = harness([{ ok: true, status: 200 }], { autoStart: true });
  const eventId = environment.analytics.track("wishlist_added", { product_id: "product-1" });
  await environment.analytics.flush();
  const events = environment.requests.flatMap((request) => JSON.parse(request.options.body).events);
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
  const eventId = environment.analytics.track("coupon_applied", { code: "WELCOME" });
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

test("splits active engagement into bounded behavior events", async () => {
  const environment = harness([{ ok: true, status: 200 }]);
  environment.analytics.pageView();
  environment.advance(125_000);
  assert.equal(environment.analytics.flushViewDuration(), 125_000);
  await environment.analytics.flush();
  const events = JSON.parse(environment.requests[0]!.options.body).events;
  assert.deepEqual(
    events.filter((event: { event_name: string }) => event.event_name === "view_duration")
      .map((event: { properties: { active_milliseconds: number } }) => event.properties.active_milliseconds),
    [60_000, 60_000, 5_000],
  );
});

test("keeps one stable provider event identity", () => {
  const environment = harness([], {
    providers: { metaPixel: { pixelId: "12345" }, ga4: { measurementId: "G-TEST1234" } },
  });
  const eventId = environment.analytics.viewContent({
    productId: "00000000-0000-4000-8000-000000000200",
    productVariantId: "00000000-0000-4000-8000-000000000100",
  });
  const metaTrack = (environment.window as unknown as { fbq: { queue: unknown[][] } }).fbq.queue.find(
    (call) => call[0] === "track" && call[1] === "ViewContent",
  );
  assert.deepEqual(metaTrack?.[3], { eventID: eventId });
});
