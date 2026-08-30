use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    fulfillment::{FulfillmentId, FulfillmentStatus, ShippingProviderAccountId},
    integration::{PaymentProvider, ShippingProvider},
    payments::{PaymentAttemptStatus, RefundId, RefundStatus},
    pricing::PriceListId,
    sales::{
        CartId, CartStatus, OrderId, OrderIdentity, OrderPaymentStatus, OrderShippingStatus,
        OrderStatus, ShopperId,
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
    pub version: u64,
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
/// partial refunds — see `commerce.order_refunds`.
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
/// `commerce.order_fulfillments`.
pub struct OrderFulfillmentItem {
    pub id: FulfillmentId,
    pub shipping_provider_account_id: ShippingProviderAccountId,
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
    pub shipping_status: OrderShippingStatus,
    pub payment_provider: Option<PaymentProvider>,
    pub payment_provider_reference_id: Option<String>,
    pub shipping_provider: Option<ShippingProvider>,
    pub shipping_provider_reference_id: Option<String>,
    pub identity: OrderIdentity,
    pub subtotal_amount_minor: i64,
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub shipping_amount_minor: i64,
    pub total_amount_minor: i64,
    pub refunded_amount_minor: i64,
    pub lines: Vec<OrderLineItem>,
    pub payment_attempt: Option<OrderPaymentAttemptItem>,
    pub refunds: Vec<OrderRefundItem>,
    pub fulfillments: Vec<OrderFulfillmentItem>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
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
