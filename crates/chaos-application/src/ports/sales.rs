use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    inventory::InventoryReservationId,
    pricing::PriceListId,
    sales::{CartId, CartStatus, CheckoutId, CheckoutIdentity, OrderId, OrderStatus, ShopperId},
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApplicationError, merchant::MerchantActor};

use super::{IdempotencyRequest, ShopperActor};

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
    pub tax_inclusive: bool,
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

pub struct CheckoutLineItem {
    pub product_id: ProductId,
    pub product_variant_id: ProductVariantId,
    pub product_title: String,
    pub variant_title: String,
    pub sku: Option<String>,
    pub requires_shipping: bool,
    pub quantity: u32,
    pub unit_price_amount_minor: i64,
    pub subtotal_amount_minor: i64,
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub total_amount_minor: i64,
    pub tax_inclusive: bool,
}

pub struct CheckoutDetail {
    pub id: CheckoutId,
    pub shopper_id: ShopperId,
    pub cart_id: CartId,
    pub inventory_reservation_id: Option<InventoryReservationId>,
    pub price_list_id: PriceListId,
    pub currency: CurrencyCode,
    pub status: String,
    pub identity: CheckoutIdentity,
    pub subtotal_amount_minor: i64,
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub total_amount_minor: i64,
    pub expires_at: OffsetDateTime,
    pub lines: Vec<CheckoutLineItem>,
    pub created_at: OffsetDateTime,
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
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub total_amount_minor: i64,
    pub tax_inclusive: bool,
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
    pub shopper_id: ShopperId,
    pub checkout_id: CheckoutId,
    pub inventory_reservation_id: Option<InventoryReservationId>,
    pub price_list_id: PriceListId,
    pub currency: CurrencyCode,
    pub status: OrderStatus,
    pub identity: CheckoutIdentity,
    pub subtotal_amount_minor: i64,
    pub discount_amount_minor: i64,
    pub tax_amount_minor: i64,
    pub total_amount_minor: i64,
    pub lines: Vec<OrderLineItem>,
    pub transitions: Vec<OrderTransitionItem>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckoutExpiryJob {
    pub id: CheckoutId,
    pub merchant_account_id: Uuid,
    pub store_id: Uuid,
    pub inventory_reservation_id: Option<InventoryReservationId>,
}

#[async_trait]
pub trait StorefrontSalesRepository: Send + Sync {
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

    async fn create_checkout(
        &self,
        actor: &ShopperActor,
        cart_id: CartId,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
        identity: CheckoutIdentity,
        idempotency: &IdempotencyRequest,
    ) -> Result<CheckoutDetail, ApplicationError>;

    async fn get_checkout(
        &self,
        actor: &ShopperActor,
        checkout_id: CheckoutId,
    ) -> Result<Option<CheckoutDetail>, ApplicationError>;

    async fn create_order(
        &self,
        actor: &ShopperActor,
        checkout_id: CheckoutId,
        now: OffsetDateTime,
        idempotency: &IdempotencyRequest,
    ) -> Result<OrderDetail, ApplicationError>;

    async fn get_order(
        &self,
        actor: &ShopperActor,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError>;
}

#[async_trait]
pub trait CheckoutExpiryQueue: Send + Sync {
    async fn claim_due_checkouts(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<CheckoutExpiryJob>, ApplicationError>;

    async fn expire_checkout(
        &self,
        worker_id: Uuid,
        job: CheckoutExpiryJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}

#[async_trait]
pub trait OrderManagementRepository: Send + Sync {
    async fn get_order(
        &self,
        actor: MerchantActor,
        store_id: chaos_domain::merchant::StoreId,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError>;

    async fn transition_order(
        &self,
        actor: MerchantActor,
        store_id: chaos_domain::merchant::StoreId,
        order_id: OrderId,
        target_status: OrderStatus,
        now: OffsetDateTime,
        idempotency: &IdempotencyRequest,
    ) -> Result<OrderDetail, ApplicationError>;
}
