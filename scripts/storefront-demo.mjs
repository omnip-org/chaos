#!/usr/bin/env node

import { chmod, writeFile } from "node:fs/promises";
import process from "node:process";

const apiOrigin = (process.env.CHAOS_API_ORIGIN ?? "http://127.0.0.1:8080").replace(/\/$/, "");
const command = process.argv[2] ?? "setup";
const orderId = process.argv[3];
const runId = process.env.CHAOS_DEMO_RUN_ID ?? Date.now().toString(36);
let accessKey = process.env.CHAOS_ACCESS_KEY;
let storeId = process.env.CHAOS_STORE_ID;
let requestId = 0;

function required(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

async function api(path, init = {}) {
  const response = await fetch(`${apiOrigin}${path}`, {
    ...init,
    headers: { "content-type": "application/json", ...init.headers },
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(`${init.method ?? "GET"} ${path} failed (${response.status}): ${JSON.stringify(body)}`);
  }
  return body.data;
}

async function mcp(tool, arguments_, selectedStore = storeId) {
  const headers = {
    accept: "application/json, text/event-stream",
    authorization: `Bearer ${requiredAccessKey()}`,
    "content-type": "application/json",
    "mcp-protocol-version": "2025-03-26",
  };
  if (selectedStore) headers["x-chaos-store-id"] = selectedStore;
  const response = await fetch(`${apiOrigin}/mcp/v1`, {
    method: "POST",
    headers,
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: ++requestId,
      method: "tools/call",
      params: { name: tool, arguments: arguments_ },
    }),
  });
  const rpc = await response.json().catch(async () => ({ raw: await response.text() }));
  if (!response.ok || rpc.error) {
    throw new Error(`MCP ${tool} failed (${response.status}): ${JSON.stringify(rpc.error ?? rpc)}`);
  }
  if (rpc.result?.isError) {
    throw new Error(`MCP ${tool} failed: ${JSON.stringify(rpc.result)}`);
  }
  const text = rpc.result?.content?.find((item) => item.type === "text")?.text;
  if (!text) return rpc.result?.structuredContent ?? {};
  const value = JSON.parse(text);
  if (value?.code && value?.message) throw new Error(`MCP ${tool} failed: ${value.code}: ${value.message}`);
  return value;
}

function requiredAccessKey() {
  if (!accessKey) throw new Error("CHAOS_ACCESS_KEY is required");
  return accessKey;
}

async function createIdentityCredentials() {
  if (accessKey) return;
  let jwt = process.env.CHAOS_USER_JWT;
  if (!jwt) {
    const grant = await api("/identity/v1/auth/external", {
      method: "POST",
      body: JSON.stringify({
        provider: required("CHAOS_IDENTITY_PROVIDER"),
        identity_token: required("CHAOS_IDENTITY_TOKEN"),
      }),
    });
    jwt = grant.access_token;
  }
  const created = await api("/identity/v1/access-keys", {
    method: "POST",
    headers: { authorization: `Bearer ${jwt}` },
    body: JSON.stringify({ name: `Storefront demo ${runId}` }),
  });
  accessKey = created.secret;
}

async function setup() {
  await createIdentityCredentials();
  const code = process.env.CHAOS_DEMO_STORE_CODE ?? `chaos-demo-${runId}`;
  const currency = process.env.CHAOS_DEMO_CURRENCY ?? "USD";
  const shippingCountries = (process.env.CHAOS_DEMO_SHIPPING_COUNTRIES ?? "US,SG,CN")
    .split(",")
    .map((country) => country.trim().toUpperCase())
    .filter(Boolean);
  const store = await mcp("create_store", {
    code,
    name: process.env.CHAOS_DEMO_STORE_NAME ?? "Chaos Demo Store",
    default_region: process.env.CHAOS_DEMO_REGION ?? "US",
    default_currency: currency,
    confirm: true,
    idempotency_key: `${runId}-store`,
  }, null);
  storeId = store.id;
  await mcp("activate_store", { confirm: true, idempotency_key: `${runId}-activate-store` });
  const channels = await mcp("list_sales_channels", {});
  const salesChannelId = channels.items[0]?.id;
  if (!salesChannelId) throw new Error("The Store has no default sales channel");

  const product = await mcp("create_product", {
    handle: "chaos-tshirt",
    title: "Chaos T-Shirt",
    description: "A complete Chaos storefront demonstration product.",
    options: [{ name: "Size", values: ["S", "M"] }],
    variants: ["S", "M"].map((size) => ({
      title: `Size ${size}`,
      sku: `CHAOS-TEE-${size}`,
      requires_shipping: true,
      track_inventory: true,
      selected_options: [{ option: "Size", value: size }],
    })),
    confirm: true,
    idempotency_key: `${runId}-product`,
  });
  const productDetail = await mcp("get_product", { product_id: product.id });
  await mcp("activate_product", {
    product_id: product.id,
    confirm: true,
    idempotency_key: `${runId}-activate-product`,
  });
  await mcp("create_price_list", {
    code: "default-usd",
    name: "Default USD",
    currency,
    tax_inclusive: false,
    activate: true,
    prices: productDetail.variants.map((variant, index) => ({
      product_variant_id: variant.id,
      amount_minor: index === 0 ? 2500 : 2700,
    })),
    confirm: true,
    idempotency_key: `${runId}-prices`,
  });
  const location = await mcp("create_inventory_location", {
    code: "main-warehouse",
    name: "Main Warehouse",
    confirm: true,
    idempotency_key: `${runId}-location`,
  });
  for (const variant of productDetail.variants) {
    await mcp("adjust_stock", {
      inventory_location_id: location.id,
      product_variant_id: variant.id,
      delta_quantity: 100,
      note: "Initial demo stock",
      confirm: true,
      idempotency_key: `${runId}-stock-${variant.id}`,
    });
  }
  await mcp("publish_product", {
    product_id: product.id,
    sales_channel_id: salesChannelId,
    confirm: true,
    idempotency_key: `${runId}-publish-product`,
  });
  const shipping = await mcp("create_shipping_service", {
    code: "standard-shipping",
    name: "Standard Shipping",
    currency,
    amount_minor: 500,
    estimated_min_days: 3,
    estimated_max_days: 7,
    destination_countries: shippingCountries,
    confirm: true,
    idempotency_key: `${runId}-shipping-service`,
  });
  await mcp("activate_shipping_service", {
    shipping_service_id: shipping.id,
    confirm: true,
    idempotency_key: `${runId}-activate-shipping-service`,
  });
  for (const country of shippingCountries) {
    const taxRule = await mcp("create_tax_rule", {
      code: `demo-${country.toLowerCase()}`,
      name: `${country} demo tax`,
      country_code: country,
      rate_basis_points: 0,
      confirm: true,
      idempotency_key: `${runId}-tax-${country}`,
    });
    await mcp("activate_tax_rule", {
      tax_rule_id: taxRule.id,
      confirm: true,
      idempotency_key: `${runId}-activate-tax-${country}`,
    });
  }
  const publishable = await mcp("create_publishable_key", {
    name: "Demo storefront",
    scopes: ["catalog:read", "carts:write", "checkout:write", "orders:read", "analytics:write", "reviews:write"],
    confirm: true,
    idempotency_key: `${runId}-publishable-key`,
  });

  if (process.env.STRIPE_SECRET_KEY || process.env.STRIPE_PUBLISHABLE_KEY || process.env.STRIPE_WEBHOOK_SECRET) {
    const credential = await mcp("create_provider_secret", {
      kind: "payment_credential",
      value: JSON.stringify({
        secret_key: required("STRIPE_SECRET_KEY"),
        publishable_key: required("STRIPE_PUBLISHABLE_KEY"),
      }),
      confirm: true,
    });
    const webhook = await mcp("create_provider_secret", {
      kind: "payment_webhook",
      value: required("STRIPE_WEBHOOK_SECRET"),
      confirm: true,
    });
    await mcp("create_payment_provider_account", {
      provider: "stripe_checkout",
      display_name: "Stripe Embedded Checkout",
      external_account_reference: process.env.STRIPE_ACCOUNT_REFERENCE ?? `platform:${storeId}`,
      credential_secret_reference: credential.secret_reference,
      webhook_secret_reference: webhook.secret_reference,
      enabled: true,
      confirm: true,
      idempotency_key: `${runId}-stripe-checkout`,
    });
  }

  const storefrontEnv = [
    `PUBLIC_CHAOS_PUBLISHABLE_KEY=${publishable.secret}`,
    `PUBLIC_CHAOS_STORE_API_BASE_URL=${apiOrigin}/store/v1`,
    "",
  ].join("\n");
  const adminEnv = [
    `CHAOS_API_ORIGIN=${apiOrigin}`,
    `CHAOS_ACCESS_KEY=${accessKey}`,
    `CHAOS_STORE_ID=${storeId}`,
    "",
  ].join("\n");
  await writeFile("packages/storefront-template/.env.demo", storefrontEnv, { mode: 0o600 });
  await writeFile(".env.storefront-demo", adminEnv, { mode: 0o600 });
  await chmod("packages/storefront-template/.env.demo", 0o600);
  await chmod(".env.storefront-demo", 0o600);
  console.log(JSON.stringify({
    store_id: storeId,
    product_id: product.id,
    variant_ids: productDetail.variants.map((variant) => variant.id),
    storefront_env: "packages/storefront-template/.env.demo",
    admin_env: ".env.storefront-demo",
    stripe_configured: Boolean(process.env.STRIPE_SECRET_KEY),
  }, null, 2));
}

async function fulfill() {
  if (!orderId) throw new Error("Usage: node scripts/storefront-demo.mjs fulfill <order-id>");
  requiredAccessKey();
  if (!storeId) throw new Error("CHAOS_STORE_ID is required");
  const order = await mcp("get_order", { order_id: orderId });
  if (order.status !== "confirmed") throw new Error(`Order ${orderId} is ${order.status}; wait for the verified payment webhook`);
  const allocations = order.lines
    .filter((line) => line.requires_shipping)
    .map((line) => ({ product_variant_id: line.product_variant_id, quantity: line.quantity }));
  if (allocations.length === 0) throw new Error(`Order ${orderId} has no shippable lines`);
  const fulfillment = await mcp("create_fulfillment", {
    order_id: orderId,
    allocations,
    confirm: true,
    idempotency_key: `${orderId}-fulfillment`,
  });
  await mcp("transition_fulfillment", {
    fulfillment_id: fulfillment.id,
    target_status: "shipped",
    carrier: process.env.CHAOS_DEMO_CARRIER ?? "Demo Carrier",
    tracking_number: process.env.CHAOS_DEMO_TRACKING_NUMBER ?? `DEMO-${orderId.slice(0, 8)}`,
    confirm: true,
    idempotency_key: `${orderId}-shipped`,
  });
  const delivered = await mcp("transition_fulfillment", {
    fulfillment_id: fulfillment.id,
    target_status: "delivered",
    confirm: true,
    idempotency_key: `${orderId}-delivered`,
  });
  console.log(JSON.stringify(delivered, null, 2));
}

if (command === "setup") await setup();
else if (command === "fulfill") await fulfill();
else throw new Error("Usage: node scripts/storefront-demo.mjs [setup|fulfill <order-id>]");
