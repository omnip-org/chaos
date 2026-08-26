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

export interface ProductMedia {
  id: UUID;
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

// A Store trades in exactly one currency, so a Cart never chooses one — it
// always inherits the Store's active Price List.
// eslint-disable-next-line @typescript-eslint/no-empty-interface
export interface CreateCartRequest {}

export interface CartLine {
  product_id: UUID;
  product_variant_id: UUID;
  product_title: string;
  variant_title: string;
  sku?: string;
  track_inventory: boolean;
  quantity: number;
  unit_price_amount_minor: number;
  subtotal_amount_minor: number;
  /** Current ready catalog media for storefront presentation. */
  media: ProductMedia[];
}

export interface Cart {
  id: UUID;
  price_list_id: UUID;
  currency: CurrencyCode;
  status: "active" | "completed" | "abandoned";
  version: number;
  lines: CartLine[];
  subtotal_amount_minor: number;
  created_at: string;
  updated_at: string;
}

export interface SetCartLineRequest {
  quantity: number;
}

export interface OrderContact {
  /**
   * Absent until a verified payment webhook backfills it. Stripe Embedded
   * Checkout collects the shopper's email directly when the storefront does
   * not already have one.
   */
  email?: string;
  phone?: string;
}

export interface PostalAddress {
  full_name: string;
  company?: string;
  address_line1: string;
  address_line2?: string;
  locality: string;
  administrative_area?: string;
  postal_code?: string;
  country_code: string;
}

export interface OrderLine {
  product_id: UUID;
  product_variant_id: UUID;
  product_title: string;
  variant_title: string;
  sku?: string;
  track_inventory: boolean;
  quantity: number;
  unit_price_amount_minor: number;
  subtotal_amount_minor: number;
}

/** The current payment state exposed for an Order. */
export interface OrderPaymentAttempt {
  status: "pending" | "authorized" | "captured" | "failed" | "cancelled";
  amount_minor: number;
  provider_reference_id?: string;
  failure_code?: string;
  created_at: string;
  updated_at: string;
}

/** One Refund recorded against an Order. An Order may have more than one. */
export interface OrderRefund {
  id: UUID;
  status: "pending" | "succeeded" | "failed";
  amount_minor: number;
  provider_reference_id?: string;
  failure_code?: string;
  created_at: string;
  updated_at: string;
}

/** One shipment against an Order. An Order may have more than one
 * concurrently active (non-cancelled) Fulfillment for split shipments. */
export interface OrderFulfillment {
  id: UUID;
  status: "awaiting_pickup" | "shipped" | "delivered" | "cancelled";
  tracking_number?: string;
  tracking_url?: string;
  shipped_at?: string;
  delivered_at?: string;
  cancelled_at?: string;
  created_at: string;
  updated_at: string;
}

/** The subset of a Fulfillment exposed on the order-tracking view: shipping
 * progress and carrier tracking, without the internal Store provider-account id. */
export interface TrackedOrderFulfillment {
  status: "awaiting_pickup" | "shipped" | "delivered" | "cancelled";
  tracking_number?: string;
  tracking_url?: string;
  shipped_at?: string;
  delivered_at?: string;
}

export interface Order {
  id: UUID;
  order_number: string;
  price_list_id: UUID;
  currency: CurrencyCode;
  status: "pending" | "confirmed" | "cancelled";
  payment_status: "pending" | "paid" | "failed" | "partially_refunded" | "refunded";
  shipping_status: "pending" | "awaiting_pickup" | "shipped" | "delivered" | "cancelled";
  payment_provider?: "stripe";
  payment_provider_reference_id?: string;
  shipping_provider?: "manual";
  shipping_provider_reference_id?: string;
  contact: OrderContact;
  billing_address?: PostalAddress;
  shipping_address?: PostalAddress;
  subtotal_amount_minor: number;
  discount_amount_minor: number;
  tax_amount_minor: number;
  shipping_amount_minor: number;
  total_amount_minor: number;
  refunded_amount_minor: number;
  lines: OrderLine[];
  payment_attempt?: OrderPaymentAttempt;
  refunds: OrderRefund[];
  fulfillments: OrderFulfillment[];
  created_at: string;
  updated_at: string;
}

/**
 * The order-tracking view served through the long-lived capability link.
 * Contact details and the full billing/shipping address are intentionally
 * absent — see `TrackedOrderData` on the API side.
 */
export interface TrackedOrder {
  id: UUID;
  order_number: string;
  currency: CurrencyCode;
  status: "pending" | "confirmed" | "cancelled";
  payment_status: "pending" | "paid" | "failed" | "partially_refunded" | "refunded";
  shipping_status: "pending" | "awaiting_pickup" | "shipped" | "delivered" | "cancelled";
  shipping_locality?: string;
  shipping_country_code?: string;
  subtotal_amount_minor: number;
  discount_amount_minor: number;
  tax_amount_minor: number;
  shipping_amount_minor: number;
  total_amount_minor: number;
  refunded_amount_minor: number;
  fulfillments: TrackedOrderFulfillment[];
  lines: OrderLine[];
  created_at: string;
  updated_at: string;
}

export interface CreateEmbeddedCheckoutRequest {
  /**
   * Optional: omit to let Stripe Embedded Checkout collect the shopper's
   * email directly. Pass it only if the storefront already has a verified
   * value to prefill.
   */
  email?: string;
  /** Supported payment provider selected for this checkout. */
  payment_provider: "stripe";
  /** Stripe appends the order ID to this URL before redirecting the shopper. */
  return_url: string;
}

export interface EmbeddedCheckoutSession {
  order_id: UUID;
  client_action: PaymentClientAction;
}

export interface PaymentClientAction {
  /**
   * client_token is an Embedded Checkout Session client secret. Pass it to
   * Stripe's EmbeddedCheckoutProvider.
   */
  type: "mount_embedded_checkout";
  public_key: string;
  client_token: string;
}

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

// Analytics
export interface TrafficTouchpoint {
  source?: string;
  medium?: string;
  campaign?: string;
  campaign_id?: string;
  term?: string;
  content?: string;
  referrer_domain?: string;
  fbclid?: string;
  gclid?: string;
}

export interface TrafficAttribution {
  first: TrafficTouchpoint;
  session: TrafficTouchpoint;
  last_non_direct?: TrafficTouchpoint;
}

/**
 * Known event names get editor completion; custom names remain supported but
 * are validated at runtime against the Storefront API snake_case contract.
 */
export type BrowserAnalyticsEventName =
  | "page_view"
  | "view_content"
  | "search"
  | "view_duration"
  | "add_to_cart"
  | "initiate_checkout"
  | "add_payment_info"
  | "purchase"
  | "refund"
  | (string & {});

export interface BrowserAnalyticsEvent {
  event_id: UUID;
  event_name: BrowserAnalyticsEventName;
  occurred_at: string;
  properties: Record<string, unknown>;
}

export interface AnalyticsCollectionResult {
  received: number;
  stored: number;
  duplicates: number;
}

// Pagination query shared by list endpoints.
export interface CursorPageParams {
  cursor?: string;
  limit?: number;
}
