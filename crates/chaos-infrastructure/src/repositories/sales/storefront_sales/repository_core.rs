// Storefront sales repository core imports, row shapes, wiring, and shared constants.

use std::collections::HashMap;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{
        CartDetail, CartLineItem, CheckoutDetail, CheckoutExpiryJob, CheckoutExpiryQueue,
        CheckoutLineItem, IdempotencyRequest, MachineActor, OrderDetail, OrderLineItem,
        OrderTrackingSession, OrderTransitionItem, ShopperActor, StorefrontMediaAsset,
        StorefrontSalesRepository, StripeCheckoutDraft,
    },
};
use chaos_domain::{
    CurrencyCode, Locale,
    catalog::{ProductId, ProductVariantId},
    fulfillment::{ShippingSelection, ShippingServiceId},
    inventory::{InventoryBalance, InventoryReservationId},
    pricing::{
        Money, PriceListId, Promotion, PromotionId, PromotionSnapshot, PromotionStatus,
        PromotionTrigger, PromotionValue, TaxRule, TaxRuleId, TaxRuleSnapshot, TaxRuleStatus,
    },
    sales::{
        Cart, CartId, CartLine, CartStatus, Checkout, CheckoutContact, CheckoutId,
        CheckoutIdentity, CommercialAdjustments, Order, OrderDeliveryStatus,
        OrderFulfillmentStatus, OrderId, OrderNumber, OrderStatus, PostalAddress, ShopperId,
    },
    store::{SalesChannelId, StoreId},
};
use rand::Rng;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::{
    analytics::{AnalyticsEventToAppend, append_event},
    inventory::{ReservationClosure, close_reservation},
    shared::idempotency::{self, IdempotencyScope},
};

const CREATE_CART_OPERATION: &str = "carts.create.v1";
const ORDER_NUMBER_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const TRACKING_SESSION_LIFETIME: time::Duration = time::Duration::minutes(30);

fn generate_order_number(now: OffsetDateTime) -> Result<OrderNumber, ApplicationError> {
    let mut random = [0_u8; 8];
    rand::rng().fill_bytes(&mut random);
    let suffix: String = random
        .into_iter()
        .map(|byte| char::from(ORDER_NUMBER_ALPHABET[usize::from(byte & 31)]))
        .collect();
    let date = now.date();
    OrderNumber::parse(format!(
        "W-{:04}{:02}{:02}-{suffix}",
        date.year(),
        u8::from(date.month()),
        date.day()
    ))
    .map_err(ApplicationError::from)
}
const SET_CART_LINE_OPERATION: &str = "cart_lines.set.v1";
const REMOVE_CART_LINE_OPERATION: &str = "cart_lines.remove.v1";
const CREATE_CHECKOUT_OPERATION: &str = "checkouts.create.v1";
const CREATE_ORDER_OPERATION: &str = "orders.create.v1";
const CREATE_STRIPE_CHECKOUT_OPERATION: &str = "stripe_checkouts.create.v1";

type CartHeaderRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    String,
    String,
    i64,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(sqlx::FromRow)]
struct PromotionCheckoutRow {
    id: Uuid,
    handle: String,
    name: String,
    trigger: String,
    redemption_code: Option<String>,
    value_kind: String,
    rate_basis_points: Option<i32>,
    amount_minor: Option<i64>,
    maximum_amount_minor: Option<i64>,
    minimum_subtotal_amount_minor: i64,
    priority: i16,
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
}

#[derive(sqlx::FromRow)]
struct PromotionSnapshotRow {
    promotion_id: Uuid,
    handle: String,
    name: String,
    trigger: String,
    redemption_code: Option<String>,
    value_kind: String,
    rate_basis_points: Option<i32>,
    amount_minor: Option<i64>,
    maximum_amount_minor: Option<i64>,
    currency: String,
    minimum_subtotal_amount_minor: i64,
    priority: i16,
    starts_at: Option<OffsetDateTime>,
    ends_at: Option<OffsetDateTime>,
}
type CartLineRow = (
    Uuid,
    Uuid,
    String,
    String,
    Option<String>,
    bool,
    bool,
    i32,
    i64,
    bool,
);
type CartMediaRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    String,
    String,
    String,
    i16,
    String,
);
type CheckoutHeaderRow = (
    Uuid,
    Uuid,
    Uuid,
    Option<Uuid>,
    Uuid,
    String,
    String,
    i64,
    i64,
    i64,
    bool,
    i64,
    i64,
    OffsetDateTime,
    OffsetDateTime,
);
type CheckoutLineRow = (
    Uuid,
    Uuid,
    String,
    String,
    Option<String>,
    bool,
    i32,
    i64,
    i64,
    i64,
    i64,
    i64,
    bool,
);
type OrderHeaderRow = (
    Uuid,
    Uuid,
    Uuid,
    Option<Uuid>,
    Uuid,
    String,
    String,
    i64,
    i64,
    i64,
    bool,
    i64,
    i64,
    OffsetDateTime,
    OffsetDateTime,
);
type OrderLineRow = (
    Uuid,
    Uuid,
    String,
    String,
    Option<String>,
    bool,
    bool,
    i32,
    i64,
    i64,
    i64,
    i64,
    i64,
    bool,
);
type AddressRow = (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
);

#[derive(Clone)]
pub struct PostgresStorefrontSalesRepository {
    pool: PgPool,
}

impl PostgresStorefrontSalesRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(actor.store_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }

    async fn begin_shopper(
        &self,
        shopper: &ShopperActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.begin(&shopper.machine).await?;
        sqlx::query("SELECT set_config('app.shopper_id', $1, true)")
            .bind(shopper.shopper_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query(
            "UPDATE commerce.shoppers \
             SET last_seen_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(shopper.machine.store_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }
}
