#!/usr/bin/env node
// Provisions the catalog described in catalog.json into one Chaos Store over
// MCP (Streamable HTTP), so the same declarative catalog can be applied to
// both a test Store and a live Store by pointing this script at each in turn.
//
// This is a first-time bootstrap tool, not a sync tool: it uses a stable
// idempotency key per operation derived from catalog.json's content, so
// re-running with an unchanged file safely replays the original result. If
// catalog.json changes after a Store has already been provisioned, the
// affected call fails closed with `idempotency_key_reused` rather than
// silently applying a different payload — this script does not attempt to
// reconcile an already-provisioned Store with a changed definition.
//
// What this script does NOT cover, because no MCP tool exists for it yet:
// Shipping Services, Tax Rules, and Payment Provider Accounts. See README.md
// for the manual Admin API steps for those, and for how to obtain the three
// required environment variables below.
//
// Required environment variables:
//   CHAOS_BASE_URL        e.g. https://api.example.com  (no trailing slash)
//   CHAOS_SECRET_KEY      a Secret API key for the target Store, scoped to
//                         mcp:tools, products:read, products:write,
//                         collections:write, pricing:write
//   CHAOS_SALES_CHANNEL_ID the target Store's default Web Sales Channel UUID
// Optional:
//   CATALOG_FILE          path to the catalog definition (default: ./catalog.json)

import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

const here = path.dirname(fileURLToPath(import.meta.url));

function requiredEnv(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`Missing required environment variable: ${name}`);
    process.exit(1);
  }
  return value;
}

function idempotencyKey(operation, ...parts) {
  const hash = createHash("sha256").update(JSON.stringify(parts)).digest("hex").slice(0, 16);
  return `provision:${operation}:${hash}`;
}

function slugify(value) {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/(^-|-$)/g, "");
}

/** Cartesian product of every option's values, in declared option order. */
function generateVariants(product) {
  const combinations = product.options.reduce(
    (acc, option) =>
      acc.flatMap((prefix) => option.values.map((value) => [...prefix, { option: option.name, value }])),
    [[]],
  );
  return combinations.map((selectedOptions) => ({
    title: selectedOptions.map((selection) => selection.value).join(" / "),
    sku: [product.skuPrefix, ...selectedOptions.map((selection) => slugify(selection.value))].join("-"),
    requires_shipping: true,
    track_inventory: true,
    selected_options: selectedOptions,
  }));
}

async function callTool(client, name, args) {
  const result = await client.callTool({ name, arguments: args });
  const text = result.content?.find((entry) => entry.type === "text")?.text;
  const parsed = text ? JSON.parse(text) : undefined;
  if (result.isError) {
    const message = parsed?.error ?? text ?? `${name} failed`;
    throw new Error(`${name}: ${message}`);
  }
  return parsed;
}

async function main() {
  const baseUrl = requiredEnv("CHAOS_BASE_URL");
  const secretKey = requiredEnv("CHAOS_SECRET_KEY");
  const salesChannelId = requiredEnv("CHAOS_SALES_CHANNEL_ID");
  const catalogPath = process.env.CATALOG_FILE ?? path.join(here, "catalog.json");
  const catalog = JSON.parse(await readFile(catalogPath, "utf8"));

  const transport = new StreamableHTTPClientTransport(new URL(`${baseUrl}/mcp/v1`), {
    requestInit: { headers: { Authorization: `Bearer ${secretKey}` } },
  });
  const client = new Client({ name: "chaos-provision-store", version: "1.0.0" });
  await client.connect(transport);

  try {
    const { product, collection, priceList } = catalog;
    const variants = generateVariants(product);

    console.log(`Creating product "${product.handle}" with ${variants.length} variants...`);
    const created = await callTool(client, "create_product", {
      handle: product.handle,
      title: product.title,
      description: product.description,
      options: product.options,
      variants,
      confirm: true,
      idempotency_key: idempotencyKey("create_product", product),
    });
    const productId = created.id;

    console.log("Reading back variant IDs...");
    const detail = await callTool(client, "get_product", { product_id: productId });
    const variantIdByTitle = new Map(detail.variants.map((variant) => [variant.title, variant.id]));
    const missing = variants.filter((variant) => !variantIdByTitle.has(variant.title));
    if (missing.length > 0) {
      throw new Error(
        `get_product did not return every variant just created (missing: ${missing.map((v) => v.title).join(", ")})`,
      );
    }

    console.log("Activating and publishing the product...");
    await callTool(client, "activate_product", {
      product_id: productId,
      confirm: true,
      idempotency_key: idempotencyKey("activate_product", product.handle),
    });
    await callTool(client, "publish_product", {
      product_id: productId,
      sales_channel_id: salesChannelId,
      confirm: true,
      idempotency_key: idempotencyKey("publish_product", product.handle, salesChannelId),
    });

    console.log(`Creating collection "${collection.handle}"...`);
    const createdCollection = await callTool(client, "create_collection", {
      handle: collection.handle,
      title: collection.title,
      description: collection.description,
      confirm: true,
      idempotency_key: idempotencyKey("create_collection", collection),
    });
    const collectionId = createdCollection.id;

    await callTool(client, "activate_collection", {
      collection_id: collectionId,
      confirm: true,
      idempotency_key: idempotencyKey("activate_collection", collection.handle),
    });
    await callTool(client, "add_products_to_collection", {
      collection_id: collectionId,
      product_ids: [productId],
      confirm: true,
      idempotency_key: idempotencyKey("add_products_to_collection", collection.handle, productId),
    });
    await callTool(client, "publish_collection", {
      collection_id: collectionId,
      sales_channel_id: salesChannelId,
      confirm: true,
      idempotency_key: idempotencyKey("publish_collection", collection.handle, salesChannelId),
    });

    console.log(`Creating and activating price list "${priceList.code}"...`);
    await callTool(client, "create_price_list", {
      code: priceList.code,
      name: priceList.name,
      currency: priceList.currency,
      tax_inclusive: priceList.taxInclusive,
      activate: true,
      prices: variants.map((variant) => ({
        product_variant_id: variantIdByTitle.get(variant.title),
        amount_minor: product.priceAmountMinor,
      })),
      confirm: true,
      idempotency_key: idempotencyKey("create_price_list", priceList, product.priceAmountMinor, [
        ...variantIdByTitle.values(),
      ]),
    });

    console.log("Done. Remaining manual steps (Shipping Service, Tax Rule, Payment Provider");
    console.log("Account) are not covered by this script — see README.md.");
  } finally {
    await client.close();
  }
}

main().catch((error) => {
  console.error(error.message ?? error);
  process.exit(1);
});
