use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    fulfillment::{FulfillmentId, FulfillmentProviderAccountId, FulfillmentStatus},
    integration::{FulfillmentProvider, PaymentProvider},
    payments::{PaymentAttemptStatus, RefundId, RefundStatus},
    pricing::PriceListId,
    sales::{
        CartId, CartStatus, OrderId, OrderIdentity, OrderPaymentStatus, OrderStatus, ShopperId,
    },
};
use time::OffsetDateTime;

use super::StorefrontMediaAsset;

pub struct CartLineItem {
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub product_title: String,
    pub variant_title: String,
    pub sku: Option<String>,
    pub track_inventory: bool,
    pub quantity: u32,
    pub unit_price_amount_minor: i64,
    pub subtotal_amount_minor: i64,
    /// Current ready catalog media for storefront presentation only.
    pub media: Vec<StorefrontMediaAsset>,
}

pub struct CartDetail {
    pub id: CartId,
    pub shopper_id: ShopperId,
    pub price_list_id: PriceListId,
    pub currency: CurrencyCode,
    pub status: CartStatus,
    pub lines: Vec<CartLineItem>,
    pub subtotal_amount_minor: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CheckoutDraft {
    pub order_id: OrderId,
    pub source_cart_id: CartId,
    pub currency: CurrencyCode,
    pub subtotal_amount_minor: i64,
    /// The InitiateCheckout Meta CAPI event id (== `order_id`, see
    /// `sales_commands.rs`), returned so the browser Pixel projection can
    /// reuse it and let Meta deduplicate the two copies.
    pub event_id: uuid::Uuid,
}

pub struct OrderLineItem {
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub product_title: String,
    pub variant_title: String,
    pub sku: Option<String>,
    pub track_inventory: bool,
    pub quantity: u32,
    pub unit_price_amount_minor: i64,
    pub subtotal_amount_minor: i64,
}

/// The Order's payment state, if checkout has started. The client handoff is
/// deliberately not part of the Order detail; it is a private field on the
/// source Cart and is returned only from the checkout handoff endpoints.
pub struct OrderPaymentAttemptItem {
    pub status: PaymentAttemptStatus,
    pub amount_minor: i64,
    pub provider_reference_id: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// One Refund against an Order. An Order may have more than one across
/// partial refunds — see `chaos_commerce.order_refunds`.
pub struct OrderRefundItem {
    pub id: RefundId,
    pub status: RefundStatus,
    pub amount_minor: i64,
    pub provider_reference_id: Option<String>,
    pub failure_code: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// One shipment against an Order. Kept as its own row (rather than flat
/// columns on `orders`) so the shipping history is a real timeline — see
/// `chaos_commerce.order_fulfillments`.
pub struct OrderFulfillmentItem {
    pub id: FulfillmentId,
    pub provider_account_id: FulfillmentProviderAccountId,
    pub shipping_provider: FulfillmentProvider,
    pub provider_reference_id: Option<String>,
    pub status: FulfillmentStatus,
    pub tracking_number: Option<String>,
    pub tracking_url: Option<String>,
    pub shipped_at: Option<OffsetDateTime>,
    pub delivered_at: Option<OffsetDateTime>,
    pub cancelled_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct OrderDetail {
    pub id: OrderId,
    pub order_number: chaos_domain::sales::OrderNumber,
    pub shopper_id: ShopperId,
    pub price_list_id: PriceListId,
    pub currency: CurrencyCode,
    pub status: OrderStatus,
    pub payment_status: OrderPaymentStatus,
    /// Aggregate projection of the Order's active Fulfillments.
    pub fulfillment_status: FulfillmentStatus,
    pub payment_provider: Option<PaymentProvider>,
    pub payment_provider_reference_id: Option<String>,
    pub identity: OrderIdentity,
    pub subtotal_amount_minor: i64,
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub shipping_amount_minor: i64,
    pub total_amount_minor: i64,
    /// `None` while the pending checkout still awaits the provider's final
    /// tax, discount, shipping, and total snapshot.
    pub amounts_finalized_at: Option<OffsetDateTime>,
    pub refunded_amount_minor: i64,
    pub lines: Vec<OrderLineItem>,
    pub payment_attempt: Option<OrderPaymentAttemptItem>,
    pub refunds: Vec<OrderRefundItem>,
    pub fulfillments: Vec<OrderFulfillmentItem>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Storefront response DTO for the persisted `chaos_commerce.orders` columns.
#[derive(sqlx::FromRow, serde::Serialize)]
pub struct ShopperOrderRow {
    pub id: uuid::Uuid,
    pub order_number: String,
    pub store_id: uuid::Uuid,
    pub channel_id: uuid::Uuid,
    pub shopper_id: uuid::Uuid,
    pub cart_id: uuid::Uuid,
    pub currency: String,
    pub status: String,
    pub payment_status: String,
    pub payment_provider_account_id: uuid::Uuid,
    pub payment_provider_reference_id: Option<String>,
    pub payment_failure_code: Option<String>,
    pub fulfillment_status: String,
    pub refunded_amount_minor: i64,
    pub subtotal_amount_minor: i64,
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub shipping_amount_minor: i64,
    pub total_amount_minor: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub amounts_finalized_at: Option<OffsetDateTime>,
    pub contact_email: Option<String>,
    pub contact_phone: Option<String>,
    pub billing_full_name: Option<String>,
    pub billing_address_line1: Option<String>,
    pub billing_address_line2: Option<String>,
    pub billing_locality: Option<String>,
    pub billing_administrative_area: Option<String>,
    pub billing_postal_code: Option<String>,
    pub billing_country_code: Option<String>,
    pub shipping_full_name: Option<String>,
    pub shipping_address_line1: Option<String>,
    pub shipping_address_line2: Option<String>,
    pub shipping_locality: Option<String>,
    pub shipping_administrative_area: Option<String>,
    pub shipping_postal_code: Option<String>,
    pub shipping_country_code: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Shopper-owned Order row plus related data needed for browser analytics.
pub struct ShopperOrderDetail {
    pub row: ShopperOrderRow,
    pub detail: OrderDetail,
}

pub struct OrderListFilter {
    pub order_number: Option<String>,
    pub status: Option<OrderStatus>,
    pub email: Option<String>,
}

pub struct OrderPage {
    pub items: Vec<OrderDetail>,
    pub has_more: bool,
}
