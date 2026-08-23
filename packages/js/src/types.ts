/**
 * Types mirror openapi/store-v1.json exactly. Keep both in sync when the
 * contract changes; field names stay snake_case to match the wire format.
 */

export type Locale = string;
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
  requires_shipping: boolean;
  track_inventory: boolean;
  on_hand_quantity: number;
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
  locale: Locale;
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
  locale: Locale;
  title: string;
  description: string;
  product_count: number;
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

export interface CreateCartRequest {
  currency?: CurrencyCode;
  locale?: Locale;
}

export interface CartLine {
  product_id: UUID;
  product_variant_id: UUID;
  product_title: string;
  variant_title: string;
  sku?: string;
  requires_shipping: boolean;
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
  locale: Locale;
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
  email: string;
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
  requires_shipping: boolean;
  track_inventory: boolean;
  quantity: number;
  unit_price_amount_minor: number;
  subtotal_amount_minor: number;
}

export interface OrderTransition {
  id: UUID;
  from_status?: "pending" | "confirmed" | "cancelled";
  to_status: "pending" | "confirmed" | "cancelled";
  kind: "created" | "confirmed" | "cancelled";
  occurred_at: string;
}

export interface Order {
  id: UUID;
  order_number: string;
  price_list_id: UUID;
  currency: CurrencyCode;
  locale: Locale;
  status: "pending" | "confirmed" | "cancelled";
  payment_status: "pending" | "paid" | "failed" | "partially_refunded" | "refunded";
  shipping_status: "pending" | "shipped" | "delivered" | "cancelled";
  contact: OrderContact;
  billing_address?: PostalAddress;
  shipping_address?: PostalAddress;
  subtotal_amount_minor: number;
  discount_amount_minor: number;
  tax_amount_minor: number;
  shipping_amount_minor: number;
  total_amount_minor: number;
  refunded_amount_minor: number;
  stripe_checkout_session_id?: string;
  stripe_payment_intent_id?: string;
  stripe_charge_id?: string;
  shipping_provider?: string;
  shipping_provider_reference?: string;
  shipping_tracking_number?: string;
  shipping_tracking_url?: string;
  lines: OrderLine[];
  transitions: OrderTransition[];
  created_at: string;
  updated_at: string;
}

export interface CreatePaymentAttemptRequest {
  /** Stripe returns the shopper here after Embedded Checkout completes. */
  return_url: string;
}

export interface CreateEmbeddedCheckoutRequest {
  email: string;
  /** Stripe appends the order ID to this URL before redirecting the shopper. */
  return_url: string;
}

export interface EmbeddedCheckoutSession {
  order_id: UUID;
}

export interface PaymentAttempt {
  id: UUID;
  order_id: UUID;
  amount_minor: number;
  currency: CurrencyCode;
  status: "pending" | "authorized" | "captured" | "failed" | "cancelled";
  stripe_checkout_session_id?: string;
  failure_code?: string;
  created_at: string;
  updated_at: string;
}

export interface PaymentClientAction {
  /**
   * "confirm_payment": client_token is a PaymentIntent client secret for
   * Stripe.js/Elements confirmation.
   * "mount_embedded_checkout": client_token is an Embedded Checkout Session
   * client secret. Pass it to Stripe's EmbeddedCheckoutProvider.
   */
  type: "confirm_payment" | "mount_embedded_checkout";
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
  code: string;
  message: string;
  details?: ErrorDetail[];
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

export type BrowserAnalyticsEventName = string;

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
