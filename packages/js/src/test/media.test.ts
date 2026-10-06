import assert from "node:assert/strict";
import test from "node:test";

import { resolveProductMedia } from "../media.js";
import type { Product, ProductMedia, ProductVariant } from "../types.js";

type MediaScopeFields =
  | { scope: "product" }
  | { scope: "option_value"; option_id: string; option_value_id: string }
  | { scope: "variant"; product_variant_id: string };

const productMedia = (
  id: string,
  position: number,
  scope: MediaScopeFields,
): ProductMedia => ({
  id,
  media_type: "image/jpeg",
  kind: "image",
  alt_text: "",
  position,
  url: `https://cdn.example/${id}.jpg`,
  ...scope,
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
    productMedia("product-image", 0, { scope: "product" }),
    productMedia("red-image", 0, {
      scope: "option_value",
      option_id: "color",
      option_value_id: "red",
    }),
    productMedia("shared-image", 1, {
      scope: "option_value",
      option_id: "color",
      option_value_id: "red",
    }),
    productMedia("shared-image", 2, {
      scope: "option_value",
      option_id: "length",
      option_value_id: "100",
    }),
    productMedia("variant-image", 0, {
      scope: "variant",
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
