import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import { sendMetaCapiEvent, type MetaCapiConfig } from "../providers/meta-capi.js";

function sha256Hex(input: string): string {
  return createHash("sha256").update(input).digest("hex");
}

function harness() {
  const requests: Array<{ url: URL; body: Record<string, unknown> }> = [];
  const config: MetaCapiConfig = {
    accessToken: "capi-token",
    pixelId: "pixel-1",
    fetch: (async (url: string, init: RequestInit) => {
      requests.push({
        url: new URL(url),
        body: JSON.parse(String(init.body)),
      });
      return {
        ok: true,
        status: 200,
        json: async () => ({ events_received: 1 }),
      } as Response;
    }) as unknown as typeof fetch,
  };
  return { config, requests };
}

test("sends AddToCart with the wire shape ported from the Rust adapter", async () => {
  const { config, requests } = harness();
  await sendMetaCapiEvent(config, {
    eventName: "add_to_cart",
    eventId: "00000000-0000-4000-8000-000000000001",
    occurredAt: new Date("2026-08-16T00:00:00Z"),
    context: {
      eventSourceUrl: "https://shop.example.com/cart",
      fbc: "fb.1.1234567890123.click",
      fbp: "fb.1.1234567890123.browser",
      clientIpAddress: "198.51.100.9",
      clientUserAgent: "Browser/1.0",
      shopperToken: "shopper-token-abc",
    },
    input: {
      cartId: "00000000-0000-4000-8000-000000000010",
      productId: "00000000-0000-4000-8000-000000000011",
      productVariantId: "00000000-0000-4000-8000-000000000012",
      quantity: 2,
      priceMinor: 500,
      valueMinor: 1_000,
      currency: "usd",
    },
  });

  assert.equal(requests.length, 1);
  const { url, body } = requests[0]!;
  assert.equal(url.origin, "https://graph.facebook.com");
  assert.equal(url.pathname, "/v21.0/pixel-1/events");
  assert.equal(url.searchParams.get("access_token"), "capi-token");

  const events = body.data as Array<Record<string, unknown>>;
  assert.equal(events.length, 1);
  const event = events[0]!;
  assert.equal(event.event_name, "AddToCart");
  assert.equal(event.event_time, Math.floor(Date.parse("2026-08-16T00:00:00Z") / 1000));
  assert.equal(event.event_id, "00000000-0000-4000-8000-000000000001");
  assert.equal(event.action_source, "website");
  assert.equal(event.event_source_url, "https://shop.example.com/cart");

  const userData = event.user_data as Record<string, unknown>;
  assert.deepEqual(userData.external_id, [sha256Hex("shopper-token-abc")]);
  assert.equal(userData.fbc, "fb.1.1234567890123.click");
  assert.equal(userData.fbp, "fb.1.1234567890123.browser");
  assert.equal(userData.client_ip_address, "198.51.100.9");
  assert.equal(userData.client_user_agent, "Browser/1.0");

  const customData = event.custom_data as Record<string, unknown>;
  assert.equal(customData.value, 10);
  assert.equal(customData.currency, "USD");
  assert.deepEqual(customData.content_ids, ["00000000-0000-4000-8000-000000000012"]);
  assert.equal(customData.content_type, "product");
  assert.equal(customData.num_items, 2);
  assert.deepEqual(customData.contents, [
    { id: "00000000-0000-4000-8000-000000000012", quantity: 2, item_price: 5 },
  ]);

  assert.equal("test_event_code" in body, false);
});

test("drops a malformed fbc/fbp instead of forwarding it", async () => {
  const { config, requests } = harness();
  await sendMetaCapiEvent(config, {
    eventName: "purchase",
    eventId: "00000000-0000-4000-8000-000000000002",
    context: { fbc: "not-a-valid-fbc", fbp: "also-invalid" },
    input: {
      orderId: "00000000-0000-4000-8000-000000000020",
      valueMinor: 1_299,
      currency: "usd",
      items: [
        {
          productId: "00000000-0000-4000-8000-000000000021",
          productVariantId: "00000000-0000-4000-8000-000000000022",
          quantity: 1,
          priceMinor: 1_299,
        },
      ],
    },
  });

  const event = (requests[0]!.body.data as Array<Record<string, unknown>>)[0]!;
  const userData = event.user_data as Record<string, unknown>;
  assert.equal("fbc" in userData, false);
  assert.equal("fbp" in userData, false);
  assert.equal("external_id" in userData, false);
});

test("uses the zero-decimal MGA currency scale for value and item_price", async () => {
  const { config, requests } = harness();
  await sendMetaCapiEvent(config, {
    eventName: "initiate_checkout",
    eventId: "00000000-0000-4000-8000-000000000003",
    input: {
      cartId: "00000000-0000-4000-8000-000000000030",
      orderNumber: "W-20260830-00000001",
      valueMinor: 1_299,
      currency: "mga",
      items: [
        {
          productId: "00000000-0000-4000-8000-000000000031",
          productVariantId: "00000000-0000-4000-8000-000000000032",
          quantity: 1,
          priceMinor: 1_299,
        },
      ],
    },
  });

  const event = (requests[0]!.body.data as Array<Record<string, unknown>>)[0]!;
  const customData = event.custom_data as Record<string, unknown>;
  assert.equal(customData.currency, "MGA");
  assert.equal(customData.value, 1_299);
  assert.deepEqual(customData.contents, [
    { id: "00000000-0000-4000-8000-000000000032", quantity: 1, item_price: 1_299 },
  ]);
});

test("includes test_event_code when configured", async () => {
  const { config, requests } = harness();
  await sendMetaCapiEvent(
    { ...config, testEventCode: "TEST12345" },
    {
      eventName: "add_to_cart",
      eventId: "00000000-0000-4000-8000-000000000004",
      input: {
        productId: "00000000-0000-4000-8000-000000000040",
        productVariantId: "00000000-0000-4000-8000-000000000041",
        quantity: 1,
        priceMinor: 100,
        valueMinor: 100,
        currency: "usd",
      },
    },
  );

  assert.equal(requests[0]!.body.test_event_code, "TEST12345");
});

test("delivery failure is swallowed, never thrown", async () => {
  const config: MetaCapiConfig = {
    accessToken: "capi-token",
    pixelId: "pixel-1",
    fetch: (async () => {
      throw new Error("network down");
    }) as unknown as typeof fetch,
  };

  await assert.doesNotReject(
    sendMetaCapiEvent(config, {
      eventName: "add_to_cart",
      eventId: "00000000-0000-4000-8000-000000000005",
      input: {
        productId: "00000000-0000-4000-8000-000000000050",
        productVariantId: "00000000-0000-4000-8000-000000000051",
        quantity: 1,
        priceMinor: 100,
        valueMinor: 100,
        currency: "usd",
      },
    }),
  );
});
