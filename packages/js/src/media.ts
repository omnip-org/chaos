import type {
  Product,
  ProductMedia,
  ProductVariant,
  UUID,
} from "./types.js";

/**
 * Resolves the gallery for one selected Product Variant.
 *
 * The API returns all active Product media rules in `product.media`. The
 * client applies the same precedence everywhere: exact Variant media,
 * matching Option Value media, then Product media. A shared physical asset
 * is emitted once even when it is attached to multiple selected values.
 */
export function resolveProductMedia(
  product: Product,
  variant: ProductVariant | UUID,
): ProductMedia[] {
  const selectedVariant =
    typeof variant === "string"
      ? product.variants.find((candidate) => candidate.id === variant)
      : variant;

  if (!selectedVariant) {
    return uniqueMedia(product.media.filter((media) => media.scope === "product"));
  }

  const exact = product.media.filter(
    (media) =>
      media.scope === "variant" &&
      media.product_variant_id === selectedVariant.id,
  );
  if (exact.length > 0) {
    return uniqueMedia(exact);
  }

  const optionValueMedia = product.media.filter(
    (media) =>
      media.scope === "option_value" &&
      selectedVariant.selected_options.some(
        (selection) =>
          selection.option_id === media.option_id &&
          selection.option_value_id === media.option_value_id,
      ),
  );
  if (optionValueMedia.length > 0) {
    return uniqueMedia(optionValueMedia);
  }

  return uniqueMedia(product.media.filter((media) => media.scope === "product"));
}

function uniqueMedia(media: ProductMedia[]): ProductMedia[] {
  const seen = new Set<UUID>();
  return media
    .filter((item) => {
      if (seen.has(item.id)) {
        return false;
      }
      seen.add(item.id);
      return true;
    })
    .sort(
      (left, right) =>
        left.position - right.position || left.id.localeCompare(right.id),
    );
}
