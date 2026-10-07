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
export type MediaKind = "image" | "video";

export interface ProductMediaBase {
  id: UUID;
  media_type: string;
  kind: MediaKind;
  alt_text: string;
  position: number;
  url: string;
}

/** A media rule with exactly the identifiers required by its attachment scope. */
export type ProductMedia = ProductMediaBase & (
  | {
      /** Product media is the final fallback. */
      scope: "product";
      option_id?: never;
      option_value_id?: never;
      product_variant_id?: never;
    }
  | {
      scope: "option_value";
      option_id: UUID;
      option_value_id: UUID;
      product_variant_id?: never;
    }
  | {
      scope: "variant";
      option_id?: never;
      option_value_id?: never;
      product_variant_id: UUID;
    }
);

/** Approved, top-level review rating for a Product — average rounded to
 * one decimal, count of rated reviews. Absent when the Product has no
 * approved reviews yet. */
export interface ProductRating {
  average: number;
  count: number;
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
  rating?: ProductRating;
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
  status: StorefrontReviewStatus;
  is_staff_reply: boolean;
  verified_buyer: boolean;
  /** Original timestamp for an imported review, when supplied by its source. */
  reviewed_at?: string;
  created_at: string;
  updated_at: string;
  replies?: Review[];
}

export type StorefrontReviewStatus = "approved";

export interface Page {
  has_more: boolean;
  next_cursor?: string;
}

export interface Meta {
  page: Page;
}

export interface ShopperSession {
  shopper_id: UUID;
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

export type CartStatus = "active" | "locked" | "completed" | "abandoned";

export interface Cart {
  id: UUID;
  currency: CurrencyCode;
  status: CartStatus;
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

export type FulfillmentStatus = "pending" | "shipped" | "delivered" | "cancelled";
export type OrderStatus = "pending" | "confirmed" | "cancelled";
export type OrderPaymentStatus =
  | "pending"
  | "paid"
  | "failed"
  | "expired"
  | "partially_refunded"
  | "refunded";

/** The subset of a Fulfillment exposed on the order-lookup view: shipping
 * progress and carrier tracking, without the internal Store provider-account id. */
export interface OrderLookupFulfillment {
  status: FulfillmentStatus;
  tracking_number?: string;
  tracking_url?: string;
  shipped_at?: string;
  delivered_at?: string;
}

/**
 * The order view returned by `orders.lookupOrder` for a matching
 * order-number + email pair. Contact details and the full billing/shipping
 * address are intentionally absent from the public lookup response.
 */
export interface OrderLookup {
  id: UUID;
  order_number: string;
  currency: CurrencyCode;
  status: OrderStatus;
  payment_status: OrderPaymentStatus;
  fulfillment_status: FulfillmentStatus;
  shipping_locality?: string | null;
  shipping_country_code?: string | null;
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

/** Minimum manual Purchase input; richer Order totals improve GA4 revenue accuracy. */
export type ConfirmedPurchaseOrderInput = Pick<
  OrderLookup,
  "id" | "status" | "payment_status" | "currency" | "total_amount_minor" | "lines"
> & Partial<Pick<
  OrderLookup,
  "subtotal_amount_minor" | "discount_amount_minor" | "tax_amount_minor" | "shipping_amount_minor"
>>;

/** Shopper-owned Order details used for confirmation UI and Purchase matching. */
export interface OwnOrder extends OrderLookup {
  contact_email: string | null;
  contact_phone: string | null;
  billing_full_name: string | null;
  billing_address_line1: string | null;
  billing_address_line2: string | null;
  billing_locality: string | null;
  billing_administrative_area: string | null;
  billing_postal_code: string | null;
  billing_country_code: string | null;
  shipping_full_name: string | null;
  shipping_address_line1: string | null;
  shipping_address_line2: string | null;
  shipping_administrative_area: string | null;
  shipping_postal_code: string | null;
  shipping_locality: string | null;
  shipping_country_code: string | null;
}

/** Polling controls for resolving a Stripe return into a terminal Order. */
export interface WaitForCheckoutOrderOptions {
  /** Delay between pending-order reads. Defaults to 1000 ms. */
  intervalMs?: number;
  /** Maximum total wait. Defaults to 30000 ms. */
  timeoutMs?: number;
  /** Cancels polling when the confirmation page is left or replaced. */
  signal?: AbortSignal;
}

/** Storefront-facing options for creating an embedded checkout. */
export interface EmbeddedCheckoutOptions {
  /** Stripe appends the Order UUID to this URL before redirecting the shopper. */
  returnUrl: string;
  /**
   * Ad-platform attribution read off the browser's own cookies/URL,
   * namespaced by platform. Defaults to reading Meta's `_fbc`/`_fbp`
   * cookies and the current page URL when omitted; pass an explicit empty
   * object to send none.
   */
  attribution?: CheckoutAttribution;
}

export type PaymentProvider = "stripe";

/** Namespaced by ad platform so a future platform is an additive field;
 * `source_url` and `utm` aren't platform-specific, so they sit alongside
 * `meta`. */
export interface CheckoutAttribution {
  source_url?: string;
  utm?: CheckoutUtm;
  meta?: { fbc?: string; fbp?: string };
}

/** Standard `utm_*` campaign tags, minus the redundant `utm_` prefix since
 * they are already namespaced under `utm`. */
export interface CheckoutUtm {
  source?: string;
  medium?: string;
  campaign?: string;
  term?: string;
  content?: string;
}

export interface EmbeddedCheckoutSession {
  order_id: UUID;
  order_number: string;
  client_action: PaymentClientAction;
}

/** The payment handoff created from an already loaded Cart snapshot. */
export interface EmbeddedCheckoutStart {
  checkout: EmbeddedCheckoutSession;
  /** The immutable source Cart snapshot used to create this checkout. */
  source_cart: Cart;
}

/** Browser-facing result of creating or recovering a checkout by Cart. */
export interface EmbeddedCheckoutCreation extends EmbeddedCheckoutStart {
  /** The newly obtained active Cart for subsequent shopping. */
  cart: Cart;
}

/** The provider-neutral client handoff needed to mount the payment form. */
export interface PaymentClientAction {
  /**
   * client_token is an Embedded Checkout Session client secret. Pass it to
   * Stripe's EmbeddedCheckoutProvider.
   */
  type: PaymentClientActionType;
  public_key: string;
  client_token: string;
}

export type PaymentClientActionType = "stripe_checkout_embedded";

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
