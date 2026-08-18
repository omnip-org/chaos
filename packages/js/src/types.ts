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
  tax_inclusive: boolean;
}

export interface ProductVariant {
  id: UUID;
  title: string;
  sku?: string;
  requires_shipping: boolean;
  price: Price;
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
  variants: ProductVariant[];
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
  tax_inclusive: boolean;
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
  /** Returned only by Cart creation. Persist and echo as x-chaos-shopper-token. */
  shopper_token?: string;
}

export interface SetCartLineRequest {
  quantity: number;
}

export interface CheckoutContact {
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

export interface CreateCheckoutRequest {
  contact: CheckoutContact;
  billing_address: PostalAddress;
  /** Required when any Cart line requires shipping. */
  shipping_address?: PostalAddress;
  /** Required for shippable Carts; revalidated when Checkout is created. */
  shipping_service_id?: UUID;
  /** Optional redemption code, revalidated when Checkout is created. */
  promotion_code?: string;
}

export interface QuoteShippingRequest {
  destination_country: string;
}

export interface ShippingOption {
  service_id: UUID;
  code: string;
  name: string;
  amount_minor: number;
  currency: CurrencyCode;
  estimated_min_days: number;
  estimated_max_days: number;
}

export interface TaxCalculation {
  rule_id: UUID;
  code: string;
  name: string;
  country_code: string;
  rate_basis_points: number;
}

export interface PromotionCalculation {
  promotion_id: UUID;
  handle: string;
  name: string;
  trigger: "automatic" | "code";
  redemption_code?: string;
  value_kind: "percentage" | "fixed_amount";
  rate_basis_points?: number;
  amount_minor?: number;
  maximum_amount_minor?: number;
  currency: CurrencyCode;
  minimum_subtotal_amount_minor: number;
  priority: number;
  starts_at?: string;
  ends_at?: string;
}

export interface CheckoutLine {
  product_id: UUID;
  product_variant_id: UUID;
  product_title: string;
  variant_title: string;
  sku?: string;
  requires_shipping: boolean;
  track_inventory?: boolean;
  quantity: number;
  unit_price_amount_minor: number;
  subtotal_amount_minor: number;
  discount_amount_minor: number;
  tax_amount_minor: number;
  total_amount_minor: number;
  tax_inclusive: boolean;
}

export interface Checkout {
  id: UUID;
  cart_id: UUID;
  customer_id?: UUID;
  inventory_reservation_id?: UUID;
  price_list_id: UUID;
  currency: CurrencyCode;
  locale: Locale;
  status: "pending" | "completed" | "expired";
  contact: CheckoutContact;
  billing_address: PostalAddress;
  shipping_address?: PostalAddress;
  shipping?: ShippingOption;
  subtotal_amount_minor: number;
  discount_amount_minor: number;
  tax_amount_minor: number;
  tax_rule: TaxCalculation;
  promotion?: PromotionCalculation;
  tax_inclusive: boolean;
  shipping_amount_minor: number;
  total_amount_minor: number;
  expires_at: string;
  lines: CheckoutLine[];
  created_at: string;
}

export type OrderLine = CheckoutLine & { track_inventory: boolean };

export interface OrderTransition {
  id: UUID;
  from_status?: "pending" | "confirmed" | "cancelled";
  to_status: "pending" | "confirmed" | "cancelled";
  kind: "created" | "confirmed" | "cancelled";
  occurred_at: string;
}

export interface Order {
  id: UUID;
  checkout_id: UUID;
  customer_id?: UUID;
  inventory_reservation_id?: UUID;
  price_list_id: UUID;
  currency: CurrencyCode;
  locale: Locale;
  status: "pending" | "confirmed" | "cancelled";
  fulfillment_status: "unfulfilled" | "partially_fulfilled" | "fulfilled";
  delivery_status: "not_delivered" | "partially_delivered" | "delivered";
  contact: CheckoutContact;
  billing_address: PostalAddress;
  shipping_address?: PostalAddress;
  shipping?: ShippingOption;
  subtotal_amount_minor: number;
  discount_amount_minor: number;
  tax_amount_minor: number;
  tax_rule: TaxCalculation;
  promotion?: PromotionCalculation;
  tax_inclusive: boolean;
  shipping_amount_minor: number;
  total_amount_minor: number;
  lines: OrderLine[];
  transitions: OrderTransition[];
  created_at: string;
  updated_at: string;
}

export interface UpdateCustomerRequest {
  phone?: string | null;
}

export interface CreateCustomerAddressRequest {
  label: string;
  full_name: string;
  company?: string;
  address_line1: string;
  address_line2?: string;
  locality: string;
  administrative_area?: string;
  postal_code?: string;
  country_code: string;
}

export interface CustomerAddress {
  id: UUID;
  label: string;
  full_name: string;
  company?: string;
  address_line1: string;
  address_line2?: string;
  locality: string;
  administrative_area?: string;
  postal_code?: string;
  country_code: string;
  created_at: string;
  updated_at: string;
}

export interface Customer {
  id: UUID;
  email: string;
  phone?: string;
  addresses: CustomerAddress[];
  created_at: string;
  updated_at: string;
}

export interface CreatePaymentAttemptRequest {
  provider: string;
  /** Required for the stripe_checkout provider (must be https://). */
  success_url?: string;
  /** Required for the stripe_checkout provider (must be https://). */
  cancel_url?: string;
}

export interface PaymentAttempt {
  id: UUID;
  order_id: UUID;
  provider: string;
  amount_minor: number;
  currency: CurrencyCode;
  status: "pending" | "authorized" | "captured" | "failed" | "cancelled";
  provider_reference?: string;
  failure_code?: string;
  created_at: string;
  updated_at: string;
}

export interface PaymentClientAction {
  provider: string;
  /**
   * "confirm_payment": client_token is a PaymentIntent client secret for
   * Stripe.js/Elements confirmation.
   * "redirect_to_checkout": client_token is the hosted Stripe Checkout
   * Session URL — navigate the shopper's browser there.
   */
  type: "confirm_payment" | "redirect_to_checkout";
  public_key: string;
  client_token: string;
  account_reference: string;
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

export interface CustomerMutationResult {
  customer_id: UUID;
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
export interface AnalyticsConsent {
  analytics_storage: boolean;
  advertising_storage: boolean;
  policy_version: string;
}

export type BrowserAnalyticsEventName =
  | "page_viewed"
  | "product_viewed"
  | "search_performed"
  | "cart_line_added"
  | "checkout_started"
  | "engagement_heartbeat";

export interface BrowserAnalyticsEvent {
  event_id: UUID;
  event_name: BrowserAnalyticsEventName;
  schema_version: 1;
  occurred_at: string;
  anonymous_id: UUID;
  session_id: UUID;
  consent: AnalyticsConsent;
  properties: Record<string, unknown>;
}

export interface AnalyticsCollectionResult {
  received: number;
  stored: number;
  duplicates: number;
  discarded_for_consent: number;
  discarded_for_policy: number;
  collection_policy_version: string;
}

// Pagination query shared by list endpoints.
export interface CursorPageParams {
  cursor?: string;
  limit?: number;
}
