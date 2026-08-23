use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    pricing::PriceListId,
    sales::{
        CartId, CartStatus, OrderId, OrderIdentity, OrderPaymentStatus, OrderShippingStatus,
        OrderStatus, ShopperId,
    },
};
use secrecy::SecretString;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ApplicationError;

use super::{AdminActor, IdempotencyRequest, MachineActor, ShopperActor, StorefrontMediaAsset};

pub struct CartLineItem {
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub product_title: String,
    pub variant_title: String,
    pub sku: Option<String>,
    pub requires_shipping: bool,
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

pub struct StripeCheckoutDraft {
    pub order_id: OrderId,
    pub currency: CurrencyCode,
    pub subtotal_amount_minor: i64,
    pub expires_at: OffsetDateTime,
}

pub struct OrderLineItem {
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub product_title: String,
    pub variant_title: String,
    pub sku: Option<String>,
    pub requires_shipping: bool,
    pub track_inventory: bool,
    pub quantity: u32,
    pub unit_price_amount_minor: i64,
    pub subtotal_amount_minor: i64,
}

pub struct OrderTransitionItem {
    pub id: uuid::Uuid,
    pub from_status: Option<OrderStatus>,
    pub to_status: OrderStatus,
    pub kind: String,
    pub actor_user_id: Option<uuid::Uuid>,
    pub occurred_at: OffsetDateTime,
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
    pub identity: OrderIdentity,
    pub subtotal_amount_minor: i64,
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub shipping_amount_minor: i64,
    pub total_amount_minor: i64,
    pub refunded_amount_minor: i64,
    pub stripe_checkout_session_id: Option<String>,
    pub stripe_payment_intent_id: Option<String>,
    pub stripe_charge_id: Option<String>,
    pub shipping_provider: Option<String>,
    pub shipping_provider_reference: Option<String>,
    pub shipping_tracking_number: Option<String>,
    pub shipping_tracking_url: Option<String>,
    pub lines: Vec<OrderLineItem>,
    pub transitions: Vec<OrderTransitionItem>,
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

#[async_trait]
pub trait StorefrontSalesRepository: Send + Sync {
    async fn create_shopper(&self, actor: &MachineActor) -> Result<ShopperId, ApplicationError>;

    async fn create_cart(
        &self,
        actor: &ShopperActor,
        currency: Option<CurrencyCode>,
        idempotency: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError>;

    async fn get_cart(
        &self,
        actor: &ShopperActor,
        cart_id: CartId,
    ) -> Result<Option<CartDetail>, ApplicationError>;

    async fn set_cart_line(
        &self,
        actor: &ShopperActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        quantity: u32,
        idempotency: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError>;

    async fn remove_cart_line(
        &self,
        actor: &ShopperActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        idempotency: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError>;

    async fn create_stripe_checkout(
        &self,
        actor: &ShopperActor,
        cart_id: CartId,
        email: &str,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
        idempotency: &IdempotencyRequest,
    ) -> Result<StripeCheckoutDraft, ApplicationError>;

    async fn get_order(
        &self,
        actor: &ShopperActor,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError>;

    async fn get_tracked_order(
        &self,
        actor: &MachineActor,
        tracking_token: &SecretString,
        now: OffsetDateTime,
    ) -> Result<Option<OrderDetail>, ApplicationError>;
}

#[async_trait]
pub trait OrderManagementRepository: Send + Sync {
    async fn list_orders(
        &self,
        actor: AdminActor,
        store_id: chaos_domain::store::StoreId,
        after: Option<Uuid>,
        limit: u16,
        filter: &OrderListFilter,
    ) -> Result<OrderPage, ApplicationError>;

    async fn get_order(
        &self,
        actor: AdminActor,
        store_id: chaos_domain::store::StoreId,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError>;

    async fn transition_order(
        &self,
        actor: AdminActor,
        store_id: chaos_domain::store::StoreId,
        order_id: OrderId,
        target_status: OrderStatus,
        now: OffsetDateTime,
        idempotency: &IdempotencyRequest,
    ) -> Result<OrderDetail, ApplicationError>;
}
