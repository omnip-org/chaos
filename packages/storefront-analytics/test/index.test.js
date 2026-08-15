import assert from "node:assert/strict";
import test from "node:test";

import { createStorefrontAnalytics } from "../src/index.js";

class FakeTarget {
  constructor() {
    this.listeners = new Map();
  }

  addEventListener(name, listener) {
    const listeners = this.listeners.get(name) ?? new Set();
    listeners.add(listener);
    this.listeners.set(name, listeners);
  }

  removeEventListener(name, listener) {
    this.listeners.get(name)?.delete(listener);
  }

  dispatch(name) {
    for (const listener of this.listeners.get(name) ?? []) listener();
  }
}

class MemoryStorage {
  constructor() {
    this.values = new Map();
  }

  getItem(key) {
    return this.values.get(key) ?? null;
  }

  setItem(key, value) {
    this.values.set(key, value);
  }
}

function harness(responses = [{ ok: true, status: 200 }]) {
  let time = Date.parse("2026-08-16T00:00:00Z");
  let sequence = 0;
  let focused = true;
  const requests = [];
  const document = Object.assign(new FakeTarget(), {
    visibilityState: "visible",
    title: "Catalog",
    referrer: "https://search.example/results?q=private",
    location: {
      pathname: "/products",
      search: "?utm_source=Newsletter&utm_medium=email&utm_campaign=Launch&ignored=secret",
    },
    hasFocus: () => focused,
  });
  const window = Object.assign(new FakeTarget(), {
    localStorage: new MemoryStorage(),
    sessionStorage: new MemoryStorage(),
  });
  const analytics = createStorefrontAnalytics({
    publishableKey: "pk_test",
    document,
    window,
    now: () => time,
    randomUUID: () => `00000000-0000-4000-8000-${String(++sequence).padStart(12, "0")}`,
    setInterval: () => 1,
    clearInterval: () => {},
    fetch: async (url, options) => {
      requests.push({ url, options });
      const response = responses.shift() ?? { ok: true, status: 200 };
      return { ...response, json: async () => ({ data: {} }) };
    },
  });
  return {
    analytics,
    document,
    window,
    requests,
    advance: (milliseconds) => { time += milliseconds; },
    focus: (value) => { focused = value; },
  };
}

test("does not queue or transmit behavior without analytics storage consent", async () => {
  const { analytics, requests } = harness();
  assert.equal(analytics.pageViewed(), null);
  assert.equal(await analytics.flush(), null);
  assert.equal(requests.length, 0);
});

test("counts only visible and focused engagement and omits full referrer URLs", async () => {
  const environment = harness();
  const { analytics, document, window, requests, advance, focus } = environment;
  analytics.setConsent({
    analyticsStorage: true,
    advertisingStorage: false,
    policyVersion: "cmp-v1",
  });
  analytics.start();
  const pageViewEventId = analytics.pageViewed();
  advance(15_000);
  focus(false);
  window.dispatch("blur");
  advance(30_000);
  document.visibilityState = "hidden";
  document.dispatch("visibilitychange");
  analytics.flushEngagement();
  await analytics.flush();

  const events = JSON.parse(requests[0].options.body).events;
  assert.equal(events[0].event_name, "page_viewed");
  assert.equal(events[0].properties.referrer_domain, "search.example");
  assert.equal(events[0].properties.campaign_source, "Newsletter");
  assert.equal(events[0].properties.campaign_medium, "email");
  assert.equal(events[0].properties.campaign_name, "Launch");
  assert.equal(JSON.stringify(events).includes("q=private"), false);
  assert.equal(JSON.stringify(events).includes("ignored"), false);
  assert.equal(events[1].event_name, "engagement_heartbeat");
  assert.equal(events[1].properties.page_view_event_id, pageViewEventId);
  assert.equal(events[1].properties.active_milliseconds, 15_000);
});

test("splits delayed active time into server-bounded heartbeat intervals", async () => {
  const { analytics, advance, requests } = harness();
  analytics.setConsent({
    analyticsStorage: true,
    advertisingStorage: true,
    policyVersion: "cmp-v2",
  });
  analytics.start();
  analytics.pageViewed({ path: "/products/example" });
  advance(125_000);
  assert.equal(analytics.flushEngagement(), 125_000);
  await analytics.flush();

  const heartbeats = JSON.parse(requests[0].options.body).events.slice(1);
  assert.deepEqual(
    heartbeats.map((event) => event.properties.active_milliseconds),
    [60_000, 60_000, 5_000],
  );
});

test("flushes the previous page engagement before an SPA navigation", async () => {
  const { analytics, advance, requests } = harness();
  analytics.setConsent({
    analyticsStorage: true,
    advertisingStorage: false,
    policyVersion: "cmp-v1",
  });
  analytics.start();
  const firstPage = analytics.pageViewed({ path: "/first" });
  advance(8_000);
  analytics.pageViewed({ path: "/second" });
  await analytics.flush();

  const events = JSON.parse(requests[0].options.body).events;
  assert.equal(events[1].event_name, "engagement_heartbeat");
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
  analytics.setConsent({
    analyticsStorage: true,
    advertisingStorage: false,
    policyVersion: "cmp-v1",
  });
  const eventId = analytics.productViewed({
    productId: "00000000-0000-4000-8000-000000000100",
  });
  await assert.rejects(analytics.flush(), /HTTP 503/);
  await analytics.flush();

  const first = JSON.parse(requests[0].options.body).events[0];
  const second = JSON.parse(requests[1].options.body).events[0];
  assert.equal(first.event_id, eventId);
  assert.deepEqual(second, first);
});

test("consent revocation drops unsent events and future engagement", async () => {
  const { analytics, advance, requests } = harness();
  analytics.setConsent({
    analyticsStorage: true,
    advertisingStorage: true,
    policyVersion: "cmp-v1",
  });
  analytics.start();
  analytics.pageViewed();
  advance(10_000);
  analytics.setConsent({
    analyticsStorage: false,
    advertisingStorage: false,
    policyVersion: "cmp-v2",
  });
  analytics.flushEngagement();
  await analytics.flush();
  assert.equal(requests.length, 0);
});
