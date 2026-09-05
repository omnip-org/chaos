/**
 * Public Storefront SDK wire types. Field names stay snake_case to match the
 * API response format.
 */

export type UUID = string;
export type CurrencyCode = string;

export interface Price {
  amount_minor: number;
  currency: CurrencyCode;
}

export interface ProductOptionValue {
  id: UUID;
  value: string;
  position: number;
}

export interface ProductOption {
  id: UUID;
  name: string;
  position: number;
  values: ProductOptionValue[];
}

export interface ProductSelectedOption {
  option_id: UUID;
  option_value_id: UUID;
}

export interface ProductCollectionReference {
  id: UUID;
  handle: string;
  title: string;
}

export interface ProductVariant {
  id: UUID;
  title: string;
  sku?: string;
  track_inventory: boolean;
  available_quantity: number;
  price: Price;
  selected_options: ProductSelectedOption[];
  metadata?: unknown;
}

export type ProductMediaScope = "product" | "option_value" | "variant";

export interface ProductMedia {
  id: UUID;
  /** Where this Media is attached; Product media is the final fallback. */
  scope: ProductMediaScope;
  option_id?: UUID;
  option_value_id?: UUID;
  product_variant_id?: UUID;
  media_type: string;
  kind: "image" | "video";
  alt_text: string;
  position: number;
  url: string;
}

export interface Product {
  id: UUID;
  handle: string;
  title: string;
  description: string;
  media: ProductMedia[];
  options: ProductOption[];
  variants: ProductVariant[];
  collections: ProductCollectionReference[];
  metadata?: unknown;
}

export interface Collection {
  id: UUID;
  handle: string;
  title: string;
  description: string;
  product_count: number;
  metadata?: unknown;
}

export interface SubmitReviewRequest {
  /** 1-5. */
  rating: number;
  title?: string;
  content: string;
  author_name: string;
  author_email?: string;
}

/**
 * An approved Review, or a staff reply nested under one via `replies`.
 * Replies carry no `rating`. list_product_reviews only ever returns
 * approved reviews, so `status` is always "approved" there.
 */
export interface Review {
  id: UUID;
  product_id: UUID;
  parent_id?: UUID;
  author_name: string;
  rating?: number;
  title?: string;
  content: string;
  images: string[];
  status: "approved";
  is_staff_reply: boolean;
  verified_buyer: boolean;
  created_at: string;
  updated_at: string;
  replies?: Review[];
}

export interface Page {
  has_more: boolean;
  next_cursor?: string;
}

export interface Meta {
  page: Page;
}

export interface ShopperSession {
  shopper_token: string;
}

export interface CartLine {
  product_id: UUID;
  product_variant_id: UUID;
  product_title: string;
  variant_title: string;
  sku?: string;
  quantity: number;
  unit_price_amount_minor: number;
  subtotal_amount_minor: number;
  /** Current ready catalog media for storefront presentation. */
  media: ProductMedia[];
}

export interface Cart {
  id: UUID;
  currency: CurrencyCode;
  status: "active" | "locked" | "completed" | "abandoned";
  version: number;
  lines: CartLine[];
  subtotal_amount_minor: number;
  created_at: string;
  updated_at: string;
}

/** Result returned by a storefront cart-line mutation bridge. */
export interface CartLineMutation {
  cart: Cart;
  product_variant_id: UUID;
  previous_quantity: number;
  new_quantity: number;
  removed: boolean;
  /** Reserved for a caller-supplied dedup id; unset by every SDK code path today. */
  event_id?: string;
}

export interface SetCartLineRequest {
  quantity: number;
}

export interface OrderLine {
  product_id: UUID;
  product_variant_id: UUID;
  product_title: string;
  variant_title: string;
  sku?: string;
  quantity: number;
  unit_price_amount_minor: number;
  subtotal_amount_minor: number;
}

/** The subset of a Fulfillment exposed on the order-lookup view: shipping
 * progress and carrier tracking, without the internal Store provider-account id. */
export interface OrderLookupFulfillment {
  status: "awaiting_pickup" | "shipped" | "delivered" | "cancelled";
  tracking_number?: string;
  tracking_url?: string;
  shipped_at?: string;
  delivered_at?: string;
}

/**
 * The order view returned by `orders.lookupOrder` for a matching
 * order-number + email pair. Contact details and the full billing/shipping
 * address are intentionally absent — see `OrderLookupData` on the API side.
 */
export interface OrderLookup {
  id: UUID;
  order_number: string;
  currency: CurrencyCode;
  status: "pending" | "confirmed" | "cancelled";
  payment_status:
    "pending" | "paid" | "failed" | "expired" | "partially_refunded" | "refunded";
  shipping_status:
    "pending" | "awaiting_pickup" | "shipped" | "delivered" | "cancelled";
  shipping_locality?: string;
  shipping_country_code?: string;
  subtotal_amount_minor: number;
  discount_amount_minor: number;
  tax_amount_minor: number;
  shipping_amount_minor: number;
  total_amount_minor: number;
  refunded_amount_minor: number;
  fulfillments: OrderLookupFulfillment[];
  lines: OrderLine[];
  created_at: string;
  updated_at: string;
}

/** Storefront-facing options for creating an embedded checkout. */
export interface EmbeddedCheckoutOptions {
  /** Stripe appends the public order number to this URL before redirecting the shopper. */
  returnUrl: string;
  /**
   * Ad-platform attribution read off the browser's own cookies/URL,
   * namespaced by platform. Defaults to reading Meta's `_fbc`/`_fbp`
   * cookies and the current page URL when omitted; pass an explicit empty
   * object to send none.
   */
  attribution?: CheckoutAttribution;
}

/** Namespaced by ad platform so a future platform is an additive field;
 * `source_url` isn't platform-specific, so it sits alongside `meta`. */
export interface CheckoutAttribution {
  source_url?: string;
  meta?: { fbc?: string; fbp?: string };
}

export interface EmbeddedCheckoutSession {
  order_number: string;
  client_action: PaymentClientAction;
  /**
   * Chaos's own server-side Meta CAPI InitiateCheckout call already used
   * this as its event id — reuse it for the browser Pixel's own
   * InitiateCheckout so Meta can deduplicate the two.
   */
  event_id: string;
}

/** Browser-facing result of creating or recovering a checkout by Cart. */
export interface EmbeddedCheckoutCreation {
  checkout: EmbeddedCheckoutSession;
  /** The immutable source Cart snapshot used to create this checkout. */
  source_cart: Cart;
  /** The newly obtained active Cart for subsequent shopping. */
  cart: Cart;
  /** Reserved for a caller-supplied dedup id; unset by every SDK code path today. */
  event_id?: string;
}

/** The provider-neutral client handoff needed to mount the payment form. */
export interface PaymentClientAction {
  /**
   * client_token is an Embedded Checkout Session client secret. Pass it to
   * Stripe's EmbeddedCheckoutProvider.
   */
  type: "mount_embedded_checkout";
  public_key: string;
  client_token: string;
}

export type OrderConfirmationState =
  | "pending"
  | "confirmed"
  | "failed"
  | "expired"
  | "cancelled";

// Envelopes — every Store API response wraps its payload in { data } (and
// { data, meta } for paginated collections).
export interface DataEnvelope<T> {
  data: T;
}

export interface PageEnvelope<T> {
  data: T[];
  meta: Meta;
}

export interface ErrorDetail {
  field: string;
  reason: string;
}

export interface ApiErrorBody {
  error?: {
    code?: string;
    message?: string;
    details?: ErrorDetail[];
  };
}

// Pagination query shared by list endpoints.
export interface CursorPageParams {
  cursor?: string;
  limit?: number;
}
