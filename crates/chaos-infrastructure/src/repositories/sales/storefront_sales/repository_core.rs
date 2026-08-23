// Storefront sales repository core imports, row shapes, wiring, and shared constants.

use std::collections::HashMap;

use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        CartDetail, CartLineItem, IdempotencyRequest, MachineActor, OrderDetail, OrderLineItem,
        OrderTransitionItem, ShopperActor, StorefrontMediaAsset, StorefrontSalesRepository,
        StripeCheckoutDraft,
    },
};
use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    pricing::{Money, PriceListId},
    sales::{
        Cart, CartId, CartLine, CartStatus, OrderContact, OrderId, OrderNumber,
        OrderPaymentStatus, OrderShippingStatus, OrderStatus, PostalAddress, ShopperId,
        OrderIdentity,
    },
    store::SalesChannelId,
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
    shared::idempotency::{self, IdempotencyScope},
};

const CREATE_CART_OPERATION: &str = "carts.create.v1";
const ORDER_NUMBER_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

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
const CREATE_STRIPE_CHECKOUT_OPERATION: &str = "stripe_checkouts.create.v1";

type CartHeaderRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    String,
    i64,
    OffsetDateTime,
    OffsetDateTime,
);

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
#[derive(sqlx::FromRow)]
struct OrderHeaderRow {
    id: Uuid,
    shopper_id: Uuid,
    price_list_id: Uuid,
    currency: String,
    status: String,
    payment_status: String,
    shipping_status: String,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    refunded_amount_minor: i64,
    stripe_checkout_session_id: Option<String>,
    stripe_payment_intent_id: Option<String>,
    stripe_charge_id: Option<String>,
    shipping_provider: Option<String>,
    shipping_provider_reference: Option<String>,
    shipping_tracking_number: Option<String>,
    shipping_tracking_url: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}
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

// Shared ownership checks, workflow parsing, and idempotency snapshots.

async fn ensure_cart_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.carts \
         WHERE store_id = $1 AND sales_channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .bind(shopper_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if owned {
        Ok(())
    } else {
        Err(cart_not_found(cart_id))
    }
}

async fn ensure_order_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.orders \
         WHERE store_id = $1 AND sales_channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(order_id.as_uuid())
    .bind(shopper_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if owned {
        Ok(())
    } else {
        Err(order_not_found(order_id))
    }
}

async fn reserve(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Value>, ApplicationError> {
    idempotency::reserve(transaction, scope, operation, request).await
}

async fn complete(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
    status: i16,
    snapshot: Value,
) -> Result<(), ApplicationError> {
    idempotency::complete(transaction, scope, operation, request, status, snapshot).await
}

fn require_channel(actor: &MachineActor) -> Result<SalesChannelId, ApplicationError> {
    actor.sales_channel_id.ok_or(ApplicationError::Forbidden)
}

fn parse_currency(value: &str) -> Result<CurrencyCode, ApplicationError> {
    CurrencyCode::parse(value).map_err(ApplicationError::from)
}

fn format_time(value: OffsetDateTime) -> Result<String, ApplicationError> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn parse_time(value: &str) -> Result<OffsetDateTime, ApplicationError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn invalid_snapshot(error: serde_json::Error) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "invalid sales idempotency snapshot: {error}"
    ))
}

fn unexpected_conversion(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn cart_not_found(cart_id: CartId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "cart",
        id: cart_id.as_uuid().to_string(),
    }
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn cart_not_active() -> ApplicationError {
    ApplicationError::Conflict {
        code: "cart_not_active",
        message: "the Cart is no longer active",
    }
}

fn price_context_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "price_context_unavailable",
        message: "no active Price List is available for the requested currency",
    }
}

fn variant_unavailable(variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "product_variant",
        id: variant_id.as_uuid().to_string(),
    }
}

fn cart_line_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "cart_line_unavailable",
        message: "one or more Cart lines are no longer published and priced",
    }
}

fn insufficient_inventory(_variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::Conflict {
        code: "insufficient_inventory",
        message: "one or more Cart lines exceed available inventory",
    }
}

fn corrupt_sales_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains an unknown sales state"))
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    eprintln!("DEBUG SQL ERROR: {error}");
    match &error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            ApplicationError::Unavailable {
                service: "postgresql",
                source: error.into(),
            }
        }
        _ => ApplicationError::Unexpected(error.into()),
    }
}

#[derive(Deserialize, Serialize)]
struct StripeCheckoutSnapshot {
    order_id: Uuid,
    currency: String,
    subtotal_amount_minor: i64,
    expires_at: String,
}

fn stripe_checkout_snapshot(detail: &StripeCheckoutDraft) -> Result<Value, ApplicationError> {
    serde_json::to_value(StripeCheckoutSnapshot {
        order_id: detail.order_id.as_uuid(),
        currency: detail.currency.as_str().into(),
        subtotal_amount_minor: detail.subtotal_amount_minor,
        expires_at: format_time(detail.expires_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_stripe_checkout(value: Value) -> Result<StripeCheckoutDraft, ApplicationError> {
    let snapshot: StripeCheckoutSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(StripeCheckoutDraft {
        order_id: OrderId::from_uuid(snapshot.order_id),
        currency: parse_currency(&snapshot.currency)?,
        subtotal_amount_minor: snapshot.subtotal_amount_minor,
        expires_at: parse_time(&snapshot.expires_at)?,
    })
}

#[derive(Serialize, Deserialize)]
struct CartSnapshot {
    id: Uuid,
    shopper_id: Uuid,
    price_list_id: Uuid,
    currency: String,
    status: String,
    version: u64,
    lines: Vec<CartLineSnapshot>,
    subtotal_amount_minor: i64,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct CartLineSnapshot {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    sku: Option<String>,
    requires_shipping: bool,
    track_inventory: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    #[serde(default)]
    media: Vec<CartLineMediaSnapshot>,
}

#[derive(Serialize, Deserialize)]
struct CartLineMediaSnapshot {
    id: Uuid,
    product_variant_id: Option<Uuid>,
    media_type: String,
    kind: String,
    alt_text: String,
    position: u16,
    url: String,
}

fn cart_snapshot(detail: &CartDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(CartSnapshot {
        id: detail.id.as_uuid(),
        shopper_id: detail.shopper_id.as_uuid(),
        price_list_id: detail.price_list_id.as_uuid(),
        currency: detail.currency.as_str().into(),
        status: detail.status.as_str().into(),
        version: detail.version,
        lines: detail.lines.iter().map(CartLineSnapshot::from).collect(),
        subtotal_amount_minor: detail.subtotal_amount_minor,
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_cart(value: Value) -> Result<CartDetail, ApplicationError> {
    let snapshot: CartSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(CartDetail {
        id: CartId::from_uuid(snapshot.id),
        shopper_id: ShopperId::from_uuid(snapshot.shopper_id),
        price_list_id: PriceListId::from_uuid(snapshot.price_list_id),
        currency: parse_currency(&snapshot.currency)?,
        status: CartStatus::parse(&snapshot.status).ok_or_else(corrupt_sales_state)?,
        version: snapshot.version,
        lines: snapshot
            .lines
            .into_iter()
            .map(CartLineItem::try_from)
            .collect::<Result<Vec<_>, _>>()?,
        subtotal_amount_minor: snapshot.subtotal_amount_minor,
        created_at: parse_time(&snapshot.created_at)?,
        updated_at: parse_time(&snapshot.updated_at)?,
    })
}

impl From<&CartLineItem> for CartLineSnapshot {
    fn from(value: &CartLineItem) -> Self {
        Self {
            product_id: value.product_id.as_uuid(),
            product_variant_id: value.product_variant_id.as_uuid(),
            product_title: value.product_title.clone(),
            variant_title: value.variant_title.clone(),
            sku: value.sku.clone(),
            requires_shipping: value.requires_shipping,
            track_inventory: value.track_inventory,
            quantity: value.quantity,
            unit_price_amount_minor: value.unit_price_amount_minor,
            subtotal_amount_minor: value.subtotal_amount_minor,
            media: value
                .media
                .iter()
                .map(|media| CartLineMediaSnapshot {
                    id: media.id.as_uuid(),
                    product_variant_id: media.product_variant_id.map(|id| id.as_uuid()),
                    media_type: media.media_type.clone(),
                    kind: media.kind.as_str().into(),
                    alt_text: media.alt_text.clone(),
                    position: media.position,
                    url: media.url.clone(),
                })
                .collect(),
        }
    }
}

impl TryFrom<CartLineSnapshot> for CartLineItem {
    type Error = ApplicationError;

    fn try_from(value: CartLineSnapshot) -> Result<Self, Self::Error> {
        Ok(Self {
            product_id: ProductId::from_uuid(value.product_id),
            product_variant_id: ProductVariantId::from_uuid(value.product_variant_id),
            product_title: value.product_title,
            variant_title: value.variant_title,
            sku: value.sku,
            requires_shipping: value.requires_shipping,
            track_inventory: value.track_inventory,
            quantity: value.quantity,
            unit_price_amount_minor: value.unit_price_amount_minor,
            subtotal_amount_minor: value.subtotal_amount_minor,
            media: value
                .media
                .into_iter()
                .map(|media| {
                    let kind = match media.kind.as_str() {
                        "image" => chaos_domain::catalog::MediaKind::Image,
                        "video" => chaos_domain::catalog::MediaKind::Video,
                        _ => return Err(corrupt_sales_state()),
                    };
                    Ok(StorefrontMediaAsset {
                        id: chaos_domain::catalog::MediaAssetId::from_uuid(media.id),
                        product_variant_id: media
                            .product_variant_id
                            .map(chaos_domain::catalog::ProductVariantId::from_uuid),
                        media_type: media.media_type,
                        kind,
                        alt_text: media.alt_text,
                        position: media.position,
                        url: media.url,
                    })
                })
                .collect::<Result<Vec<_>, ApplicationError>>()?,
        })
    }
}
