import type { PurchaseAnalyticsInput } from "./events/types.js";
import type {
  OrderConfirmationState,
  ConfirmedPurchaseOrderInput,
  ProductOption,
  ProductVariant,
  Review,
} from "./types.js";

export type SelectedOptions = Record<string, string>;

export interface VariantSelectionOption {
  id: string;
}

export interface VariantSelectionValue {
  value: string;
}

export interface VariantSelectionVariant {
  options: Record<string, string | undefined>;
}

/** A variant with inventory disabled is purchasable; tracked stock must be positive. */
export function isVariantAvailable(
  variant: Pick<ProductVariant, "track_inventory" | "available_quantity"> | undefined,
): boolean {
  if (!variant) return false;
  return !variant.track_inventory || variant.available_quantity > 0;
}

export function getProductAvailability(
  variants: ProductVariant[] | null | undefined,
): "InStock" | "OutOfStock" {
  return (variants ?? []).some(isVariantAvailable) ? "InStock" : "OutOfStock";
}

/** Resolves a complete option selection without allowing a partial match. */
export function resolveVariant<T extends VariantSelectionVariant>(
  options: readonly VariantSelectionOption[] | readonly ProductOption[],
  variants: readonly T[],
  selectedOptions: SelectedOptions,
): T | undefined {
  if (options.some((option) => !selectedOptions[option.id])) {
    return options.length === 0 ? variants[0] : undefined;
  }

  return variants.find((variant) =>
    options.every((option) => variant.options[option.id] === selectedOptions[option.id]),
  );
}

export function selectedOptionLabel(
  options: readonly VariantSelectionOption[] | readonly ProductOption[],
  selectedOptions: SelectedOptions,
  separator = " · ",
): string {
  return options
    .map((option) => selectedOptions[option.id])
    .filter((value): value is string => Boolean(value))
    .join(separator);
}

/** Average only rated top-level reviews; replies never affect the product rating. */
export function getAverageRating(reviews: readonly Review[]): number | null {
  const rated = reviews.filter(
    (review): review is Review & { rating: number } => review.rating !== undefined,
  );
  if (rated.length === 0) return null;
  const sum = rated.reduce((total, review) => total + review.rating, 0);
  return Math.round((sum / rated.length) * 10) / 10;
}

/**
 * Order lifecycle state for status badges. A refund does not erase the
 * original confirmed purchase.
 */
export function getOrderConfirmationState(
  status: string | undefined,
  paymentStatus: string | undefined,
): OrderConfirmationState {
  if (paymentStatus === "expired") return "expired";
  if (paymentStatus === "failed") return "failed";
  if (status === "cancelled") return "cancelled";
  if (status === "confirmed") return "confirmed";
  return "pending";
}

/**
 * Builds the provider-neutral Purchase input for an Order that was paid,
 * including one later partially or fully refunded.
 */
export function toPurchaseAnalyticsInput(order: ConfirmedPurchaseOrderInput): PurchaseAnalyticsInput | null {
  if (order.status !== "confirmed" ||
      !["paid", "partially_refunded", "refunded"].includes(order.payment_status)) {
    return null;
  }

  return {
    orderId: order.id,
    valueMinor: order.total_amount_minor,
    ...(order.subtotal_amount_minor !== undefined
      ? { ga4ValueMinor: order.subtotal_amount_minor - (order.discount_amount_minor ?? 0) }
      : {}),
    ...(order.tax_amount_minor !== undefined ? { taxMinor: order.tax_amount_minor } : {}),
    ...(order.shipping_amount_minor !== undefined ? { shippingMinor: order.shipping_amount_minor } : {}),
    currency: order.currency,
    items: order.lines.map((line) => ({
      productId: line.product_id,
      productVariantId: line.product_variant_id,
      quantity: line.quantity,
      priceMinor: line.unit_price_amount_minor,
    })),
  };
}
