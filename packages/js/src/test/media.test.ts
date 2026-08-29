import assert from "node:assert/strict";
import test from "node:test";

import { resolveProductMedia } from "../media.js";
import type { Product, ProductMedia, ProductVariant } from "../types.js";

const productMedia = (
  id: string,
  scope: ProductMedia["scope"],
  position: number,
  extra: Partial<ProductMedia> = {},
): ProductMedia => ({
  id,
  scope,
  media_type: "image/jpeg",
  kind: "image",
  alt_text: "",
  position,
  url: `https://cdn.example/${id}.jpg`,
  ...extra,
});

const variant = (
  id: string,
  selected_options: ProductVariant["selected_options"],
): ProductVariant => ({
  id,
  title: id,
  track_inventory: false,
  available_quantity: 0,
  price: { amount_minor: 100, currency: "USD" },
  selected_options,
});

const product: Product = {
  id: "product-1",
  handle: "chair",
  title: "Chair",
  description: "",
  media: [
    productMedia("product-image", "product", 0),
    productMedia("red-image", "option_value", 0, {
      option_id: "color",
      option_value_id: "red",
    }),
    productMedia("shared-image", "option_value", 1, {
      option_id: "color",
      option_value_id: "red",
    }),
    productMedia("shared-image", "option_value", 2, {
      option_id: "length",
      option_value_id: "100",
    }),
    productMedia("variant-image", "variant", 0, {
      product_variant_id: "red-160",
    }),
  ],
  options: [],
  variants: [
    variant("red-100", [{ option_id: "color", option_value_id: "red" }]),
    variant("blue-135", [
      { option_id: "color", option_value_id: "blue" },
      { option_id: "length", option_value_id: "135" },
    ]),
    variant("red-160", [{ option_id: "color", option_value_id: "red" }]),
  ],
  collections: [],
};

test("resolves option-value media and removes duplicate physical assets", () => {
  const media = resolveProductMedia(product, "red-100");
  assert.deepEqual(
    media.map((item) => item.id),
    ["red-image", "shared-image"],
  );
});

test("uses exact Variant media before Option Value media", () => {
  const media = resolveProductMedia(product, "red-160");
  assert.deepEqual(media.map((item) => item.id), ["variant-image"]);
});

test("falls back to Product media when no specific rule matches", () => {
  const media = resolveProductMedia(product, "blue-135");
  assert.deepEqual(media.map((item) => item.id), ["product-image"]);
});
