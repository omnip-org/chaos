import assert from "node:assert/strict";
import test from "node:test";

import { ChaosServerEvents } from "../events/server.js";
import type { MetaCapiConfig } from "../events/capi.js";

function harness() {
  const requests: Array<Record<string, unknown>> = [];
  const config: MetaCapiConfig = {
    accessToken: "capi-token",
    pixelId: "pixel-1",
    fetch: (async (_url: string, init: RequestInit) => {
      requests.push(JSON.parse(String(init.body)));
      return { ok: true, status: 200, json: async () => ({ events_received: 1 }) } as Response;
    }) as unknown as typeof fetch,
  };
  return { events: new ChaosServerEvents({ meta: config }), requests };
}

function event(requests: Array<Record<string, unknown>>, index = 0): Record<string, unknown> {
  return (requests[index]!.data as Array<Record<string, unknown>>)[0]!;
}

test("recordAddToCart mints an event ID when none is supplied", async () => {
  const { events, requests } = harness();
  const eventId = await events.recordAddToCart({
    productId: "00000000-0000-4000-8000-000000000001",
    productVariantId: "00000000-0000-4000-8000-000000000002",
    quantity: 1,
    priceMinor: 1_000,
    valueMinor: 1_000,
    currency: "usd",
  });

  assert.match(eventId, /^[0-9a-f-]{36}$/);
  assert.equal(event(requests).event_id, eventId);
  assert.equal(event(requests).event_name, "AddToCart");
});

test("recordInitiateCheckout reuses a caller-supplied event ID for Pixel/CAPI dedup", async () => {
  const { events, requests } = harness();
  const suppliedId = "00000000-0000-4000-8000-0000000000aa";
  const eventId = await events.recordInitiateCheckout(
    {
      cartId: "00000000-0000-4000-8000-000000000010",
      orderNumber: "W-20260830-00000001",
      valueMinor: 2_000,
      currency: "usd",
      items: [
        {
          productId: "00000000-0000-4000-8000-000000000011",
          productVariantId: "00000000-0000-4000-8000-000000000012",
          quantity: 1,
          priceMinor: 2_000,
        },
      ],
    },
    undefined,
    suppliedId,
  );

  assert.equal(eventId, suppliedId);
  assert.equal(event(requests).event_id, suppliedId);
});

test("recordConfirmedPurchase uses the order-derived event ID", async () => {
  const { events, requests } = harness();
  await events.recordConfirmedPurchase({
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

  assert.equal(requests.length, 1);
  assert.equal(event(requests).event_id, "00000000-0000-4000-8000-000000000099");
  assert.equal(event(requests).event_name, "Purchase");
});

test("recordConfirmedPurchase is a no-op for an order that is not confirmed and paid", async () => {
  const { events, requests } = harness();
  await events.recordConfirmedPurchase({
    id: "00000000-0000-4000-8000-000000000098",
    status: "pending",
    payment_status: "pending",
    currency: "USD",
    total_amount_minor: 2_000,
    lines: [],
  });

  assert.equal(requests.length, 0);
});
