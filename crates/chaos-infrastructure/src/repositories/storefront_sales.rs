use std::collections::HashMap;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{
        CartDetail, CartLineItem, CheckoutDetail, CheckoutExpiryJob, CheckoutExpiryQueue,
        CheckoutLineItem, IdempotencyRequest, MachineActor, OrderDetail, OrderLineItem,
        OrderTrackingSession, OrderTransitionItem, ShopperActor, StorefrontMediaAsset,
        StorefrontSalesRepository,
    },
};
use chaos_domain::{
    CurrencyCode, Locale,
    catalog::{ProductId, ProductVariantId},
    fulfillment::{ShippingSelection, ShippingServiceId},
    inventory::{InventoryReservationId, StockBalance},
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

use super::analytics::{AnalyticsEventToAppend, append_event};
use super::idempotency::{self, IdempotencyScope};
use super::inventory::{ReservationClosure, close_reservation};

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

#[async_trait]
impl StorefrontSalesRepository for PostgresStorefrontSalesRepository {
    async fn create_shopper(&self, actor: &MachineActor) -> Result<ShopperId, ApplicationError> {
        require_channel(actor)?;
        let shopper_id = ShopperId::new();
        let mut transaction = self.begin(actor).await?;
        sqlx::query("INSERT INTO commerce.shoppers (id, store_id) VALUES ($1, $2)")
            .bind(shopper_id.as_uuid())
            .bind(actor.store_id.as_uuid())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(shopper_id)
    }

    async fn create_cart(
        &self,
        shopper: &ShopperActor,
        currency: Option<CurrencyCode>,
        requested_locale: Option<Locale>,
        request: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError> {
        let shopper_id = shopper.shopper_id;
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        if let Some(snapshot) = reserve(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_CART_OPERATION,
            request,
        )
        .await?
        {
            return replay_cart(snapshot);
        }
        let (price_list_id, currency) =
            select_price_list(&mut transaction, actor, channel_id, currency)
                .await?
                .ok_or_else(price_context_unavailable)?;
        let locale = select_locale(&mut transaction, actor, requested_locale).await?;
        let cart = Cart::create(
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(price_list_id),
            currency,
        );
        sqlx::query(
            "INSERT INTO commerce.carts \
             (id, store_id, shopper_id, sales_channel_id, price_list_id, currency, locale) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(cart.id().as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(shopper_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(price_list_id)
        .bind(currency.as_str())
        .bind(locale.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = load_cart(&mut transaction, actor, cart.id())
            .await?
            .ok_or_else(|| cart_not_found(cart.id()))?;
        complete(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_CART_OPERATION,
            request,
            201,
            cart_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn get_cart(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
    ) -> Result<Option<CartDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        let detail = load_cart(&mut transaction, actor, cart_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn set_cart_line(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        quantity: u32,
        request: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        if let Some(snapshot) = reserve(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            SET_CART_LINE_OPERATION,
            request,
        )
        .await?
        {
            return replay_cart(snapshot);
        }
        let header = lock_active_cart(&mut transaction, actor, cart_id).await?;
        let previous_quantity: Option<i32> = sqlx::query_scalar(
            "SELECT quantity FROM commerce.cart_lines \
             WHERE store_id = $1 AND cart_id = $2 AND product_variant_id = $3",
        )
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .bind(product_variant_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let currency = parse_currency(&header.2)?;
        let locale = parse_locale(&header.3)?;
        let row = resolve_variant(
            &mut transaction,
            actor,
            SalesChannelId::from_uuid(header.0),
            PriceListId::from_uuid(header.1),
            product_variant_id,
            &locale,
        )
        .await?
        .ok_or_else(|| variant_unavailable(product_variant_id))?;
        let line = CartLine::new(
            ProductId::from_uuid(row.0),
            product_variant_id,
            row.1,
            row.2,
            row.3,
            row.4,
            row.5,
            quantity,
            Money::new(row.6, currency),
            row.7,
        )?;
        insert_or_replace_line(&mut transaction, actor, cart_id, &line).await?;
        bump_cart(&mut transaction, actor, cart_id).await?;
        let previous_quantity = previous_quantity
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_default();
        if quantity > previous_quantity {
            let now = OffsetDateTime::now_utc();
            append_event(
                &mut transaction,
                AnalyticsEventToAppend {
                    store_id: actor.store_id.as_uuid(),
                    shopper_id: shopper.shopper_id.as_uuid(),
                    event_id: Uuid::now_v7(),
                    event_name: "add_to_cart".into(),
                    properties: json!({
                        "_source": "server",
                        "cart_id": cart_id.as_uuid(),
                        "product_variant_id": product_variant_id.as_uuid(),
                        "quantity": quantity - previous_quantity,
                    }),
                    occurred_at: now,
                    received_at: now,
                },
            )
            .await?;
        }
        let detail = load_cart(&mut transaction, actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))?;
        complete(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            SET_CART_LINE_OPERATION,
            request,
            200,
            cart_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn remove_cart_line(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        request: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        if let Some(snapshot) = reserve(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            REMOVE_CART_LINE_OPERATION,
            request,
        )
        .await?
        {
            return replay_cart(snapshot);
        }
        lock_active_cart(&mut transaction, actor, cart_id).await?;
        sqlx::query(
            "DELETE FROM commerce.cart_lines WHERE store_id = $1 \
             AND cart_id = $2 AND product_variant_id = $3",
        )
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .bind(product_variant_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        bump_cart(&mut transaction, actor, cart_id).await?;
        let detail = load_cart(&mut transaction, actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))?;
        complete(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            REMOVE_CART_LINE_OPERATION,
            request,
            200,
            cart_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_checkout(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
        identity: CheckoutIdentity,
        shipping_service_id: Option<ShippingServiceId>,
        promotion_code: Option<&str>,
        request: &IdempotencyRequest,
    ) -> Result<CheckoutDetail, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        if let Some(snapshot) = reserve(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_CHECKOUT_OPERATION,
            request,
        )
        .await?
        {
            return replay_checkout(snapshot);
        }
        let header = lock_active_cart(&mut transaction, actor, cart_id).await?;
        if header.0 != channel_id.as_uuid() {
            return Err(cart_not_found(cart_id));
        }
        let currency = parse_currency(&header.2)?;
        let locale = parse_locale(&header.3)?;
        require_price_list_active(
            &mut transaction,
            actor,
            PriceListId::from_uuid(header.1),
            currency,
            now,
        )
        .await?;
        let lines = refresh_cart_lines(
            &mut transaction,
            actor,
            cart_id,
            channel_id,
            PriceListId::from_uuid(header.1),
            currency,
        )
        .await?;
        let existing_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commerce.cart_lines \
             WHERE store_id = $1 AND cart_id = $2",
        )
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if usize::try_from(existing_count).ok() != Some(lines.len()) {
            return Err(cart_line_unavailable());
        }
        let existing_checkout: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM commerce.checkouts \
             WHERE store_id = $1 AND cart_id = $2 AND status = 'pending' \
             FOR UPDATE",
        )
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        if existing_checkout.is_some() {
            return Err(checkout_already_pending());
        }
        let cart = Cart::rehydrate(
            cart_id,
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(header.1),
            currency,
            CartStatus::Active,
            lines,
        )?;
        let shipping = match (
            cart.lines().iter().any(CartLine::requires_shipping),
            shipping_service_id,
        ) {
            (true, Some(service_id)) => {
                let country = identity
                    .shipping_address()
                    .ok_or_else(|| ApplicationError::Validation {
                        violations: vec![chaos_domain::FieldViolation {
                            field: "shipping_address",
                            reason: "is required when the Cart contains shippable lines".into(),
                        }],
                    })?
                    .country_code();
                Some(
                    load_active_shipping_service(
                        &mut transaction,
                        actor,
                        service_id,
                        currency,
                        country,
                    )
                    .await?,
                )
            }
            (false, None) => None,
            _ => return Err(invalid_shipping_selection()),
        };
        let tax_country = if cart.lines().iter().any(CartLine::requires_shipping) {
            identity
                .shipping_address()
                .ok_or_else(invalid_shipping_selection)?
                .country_code()
        } else {
            identity.billing_address().country_code()
        };
        let tax_rule = load_active_tax_rule(&mut transaction, actor, tax_country).await?;
        let subtotals = cart
            .lines()
            .iter()
            .map(CartLine::subtotal)
            .collect::<Result<Vec<_>, _>>()?;
        let selected_promotion = select_promotion(
            &mut transaction,
            actor,
            currency,
            &subtotals,
            promotion_code,
            now,
        )
        .await?;
        let discounts = selected_promotion.as_ref().map_or_else(
            || Ok(subtotals.iter().map(|_| Money::zero(currency)).collect()),
            |(_, allocations)| Ok::<_, ApplicationError>(allocations.clone()),
        )?;
        let taxable_amounts = subtotals
            .iter()
            .zip(&discounts)
            .map(|(subtotal, discount)| subtotal.checked_sub(discount))
            .collect::<Result<Vec<_>, _>>()?;
        let tax_inclusive = cart.lines()[0].tax_inclusive();
        let taxes = tax_rule.calculate_and_allocate(&taxable_amounts, tax_inclusive)?;
        let tax_snapshot = TaxRuleSnapshot::from_rule(&tax_rule)?;
        let promotion_snapshot = selected_promotion
            .as_ref()
            .map(|(promotion, _)| PromotionSnapshot::from_promotion(promotion));
        let reservation_id =
            reserve_inventory(&mut transaction, actor, channel_id, &cart, expires_at).await?;
        let checkout = Checkout::freeze(
            &cart,
            reservation_id,
            expires_at,
            identity,
            shipping,
            tax_snapshot,
            promotion_snapshot,
            discounts
                .into_iter()
                .zip(taxes)
                .map(|(discount, tax)| CommercialAdjustments::new(discount, tax))
                .collect::<Result<Vec<_>, _>>()?,
        )?;
        insert_checkout(
            &mut transaction,
            actor,
            shopper.shopper_id,
            channel_id,
            &checkout,
            &locale,
        )
        .await?;
        append_event(
            &mut transaction,
            AnalyticsEventToAppend {
                store_id: actor.store_id.as_uuid(),
                shopper_id: shopper.shopper_id.as_uuid(),
                event_id: checkout.id().as_uuid(),
                event_name: "initiate_checkout".into(),
                properties: json!({
                    "_source": "server",
                    "cart_id": cart_id.as_uuid(),
                    "checkout_id": checkout.id().as_uuid(),
                }),
                occurred_at: now,
                received_at: now,
            },
        )
        .await?;
        let detail = load_checkout(&mut transaction, actor, checkout.id())
            .await?
            .ok_or_else(|| checkout_not_found(checkout.id()))?;
        complete(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_CHECKOUT_OPERATION,
            request,
            201,
            checkout_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn quote_shipping(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
        destination_country: &str,
    ) -> Result<Vec<ShippingSelection>, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        let header = lock_active_cart(&mut transaction, actor, cart_id).await?;
        let currency = parse_currency(&header.2)?;
        let shippable: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM commerce.cart_lines \
             WHERE store_id = $1 AND cart_id = $2 \
               AND requires_shipping)",
        )
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !shippable {
            transaction.commit().await.map_err(database_error)?;
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, (Uuid, String, String, i64, String, i16, i16)>(
            "SELECT s.id, s.code, s.name, s.amount_minor, s.currency::text, \
                    s.estimated_min_days, s.estimated_max_days \
             FROM commerce.shipping_services s \
             JOIN commerce.shipping_service_regions r \
               ON r.store_id = s.store_id AND r.shipping_service_id = s.id \
             WHERE s.store_id = $1 \
               AND s.currency = $2 AND s.status = 'active' AND r.country_code = $3 \
             ORDER BY s.amount_minor, s.code, s.id",
        )
        .bind(actor.store_id.as_uuid())
        .bind(currency.as_str())
        .bind(destination_country)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter().map(shipping_selection_from_row).collect()
    }

    async fn get_checkout(
        &self,
        shopper: &ShopperActor,
        checkout_id: CheckoutId,
    ) -> Result<Option<CheckoutDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_checkout_owner(&mut transaction, actor, checkout_id, shopper.shopper_id).await?;
        let detail = load_checkout(&mut transaction, actor, checkout_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_order(
        &self,
        shopper: &ShopperActor,
        checkout_id: CheckoutId,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<OrderDetail, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_checkout_owner(&mut transaction, actor, checkout_id, shopper.shopper_id).await?;
        if let Some(snapshot) = reserve(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_ORDER_OPERATION,
            request,
        )
        .await?
        {
            return replay_order(snapshot);
        }
        let checkout = sqlx::query_as::<_, (Uuid, String, OffsetDateTime)>(
            "SELECT cart_id, status::text, expires_at FROM commerce.checkouts \
             WHERE store_id = $1 AND sales_channel_id = $2 \
               AND id = $3 FOR UPDATE",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(checkout_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| checkout_not_found(checkout_id))?;
        if checkout.1 != "pending" {
            return Err(checkout_not_pending());
        }
        if checkout.2 <= now {
            return Err(checkout_expired());
        }
        let order = Order::create(checkout_id);
        let mut inserted = false;
        for _ in 0..5 {
            let order_number = generate_order_number(now)?;
            let result = sqlx::query(
                "INSERT INTO commerce.orders \
             (id, store_id, order_number, sales_channel_id, checkout_id, \
              shopper_id, inventory_reservation_id, price_list_id, currency, locale, subtotal_amount_minor, \
              discount_amount_minor, tax_amount_minor, tax_inclusive, shipping_amount_minor, total_amount_minor, created_at, updated_at) \
             SELECT $1, store_id, $2, sales_channel_id, id, shopper_id, \
                    inventory_reservation_id, price_list_id, currency, locale, subtotal_amount_minor, \
                    discount_amount_minor, tax_amount_minor, tax_inclusive, shipping_amount_minor, total_amount_minor, $3, $4 \
             FROM commerce.checkouts WHERE store_id = $5 \
               AND sales_channel_id = $6 AND id = $7 \
             ON CONFLICT(store_id,order_number) DO NOTHING",
            )
            .bind(order.id().as_uuid())
            .bind(order_number.as_str())
            .bind(now)
            .bind(now)
            .bind(actor.store_id.as_uuid())
            .bind(channel_id.as_uuid())
            .bind(checkout_id.as_uuid())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            if result.rows_affected() == 1 {
                inserted = true;
                break;
            }
        }
        if !inserted {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "failed to allocate a unique Order number after bounded retries"
            )));
        }
        sqlx::query(
            "INSERT INTO commerce.order_lines \
             (store_id, order_id, position, product_id, \
              product_variant_id, product_title, variant_title, sku, requires_shipping, \
              track_inventory, quantity, unit_price_amount_minor, subtotal_amount_minor, \
              discount_amount_minor, tax_amount_minor, total_amount_minor, tax_inclusive, created_at) \
             SELECT store_id, $1, position, product_id, product_variant_id, \
                    product_title, variant_title, sku, requires_shipping, track_inventory, quantity, \
                    unit_price_amount_minor, subtotal_amount_minor, discount_amount_minor, \
                    tax_amount_minor, total_amount_minor, tax_inclusive, $2 \
             FROM commerce.checkout_lines WHERE store_id = $3 \
               AND checkout_id = $4 ORDER BY position",
        )
        .bind(order.id().as_uuid())
        .bind(now)
        .bind(actor.store_id.as_uuid())
        .bind(checkout_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        copy_checkout_identity_to_order(&mut transaction, actor, checkout_id, order.id()).await?;
        copy_checkout_shipping_to_order(&mut transaction, actor, checkout_id, order.id()).await?;
        copy_checkout_tax_to_order(&mut transaction, actor, checkout_id, order.id()).await?;
        copy_checkout_promotion_to_order(&mut transaction, actor, checkout_id, order.id()).await?;
        sqlx::query(
            "INSERT INTO commerce.order_transitions \
             (id, store_id, order_id, from_status, to_status, kind, occurred_at) \
             VALUES ($1, $2, $3, NULL, 'pending', 'created', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(actor.store_id.as_uuid())
        .bind(order.id().as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "UPDATE commerce.checkouts SET status = 'completed', closed_at = $1, updated_at = $2, \
                    expiry_locked_by = NULL, expiry_locked_at = NULL \
             WHERE store_id = $3 AND sales_channel_id = $4 \
               AND id = $5 AND status = 'pending'",
        )
        .bind(now)
        .bind(now)
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(checkout_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let cart_result = sqlx::query(
            "UPDATE commerce.carts SET status = 'completed', version = version + 1, \
                    updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2 AND shopper_id = $3 AND status = 'active'",
        )
        .bind(actor.store_id.as_uuid())
        .bind(checkout.0)
        .bind(shopper.shopper_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if cart_result.rows_affected() != 1 {
            return Err(cart_not_active());
        }
        let detail = load_order(&mut transaction, actor, order.id())
            .await?
            .ok_or_else(|| order_not_found(order.id()))?;
        complete(
            &mut transaction,
            &IdempotencyScope::Shopper(shopper.shopper_id.as_uuid()),
            CREATE_ORDER_OPERATION,
            request,
            201,
            order_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn get_order(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_order_owner(&mut transaction, actor, order_id, shopper.shopper_id).await?;
        let detail = load_order(&mut transaction, actor, order_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn exchange_order_tracking_key(
        &self,
        actor: &MachineActor,
        tracking_key: &SecretString,
        now: OffsetDateTime,
    ) -> Result<Option<OrderTrackingSession>, ApplicationError> {
        if !valid_capability(tracking_key.expose_secret(), "otk_") {
            return Ok(None);
        }
        let mut transaction = self.begin(actor).await?;
        let key_digest: [u8; 32] = Sha256::digest(tracking_key.expose_secret()).into();
        let key = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT key.id, key.order_id FROM commerce.order_tracking_keys AS key \
             INNER JOIN commerce.orders AS order_row ON order_row.store_id=key.store_id AND order_row.id=key.order_id \
             WHERE key.store_id=$1 AND order_row.sales_channel_id=$2 AND key.secret_digest=$3 \
               AND key.revoked_at IS NULL AND key.expires_at>$4 FOR UPDATE OF key",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
        .bind(key_digest.as_slice())
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let Some((tracking_key_id, order_id)) = key else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let mut token_bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut token_bytes);
        let access_token =
            SecretString::from(format!("ots_{}", URL_SAFE_NO_PAD.encode(token_bytes)));
        let access_digest: [u8; 32] = Sha256::digest(access_token.expose_secret()).into();
        let expires_at = now + TRACKING_SESSION_LIFETIME;
        sqlx::query(
            "INSERT INTO commerce.order_tracking_sessions \
             (id,store_id,tracking_key_id,access_digest,expires_at,created_at) \
             VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(Uuid::now_v7())
        .bind(actor.store_id.as_uuid())
        .bind(tracking_key_id)
        .bind(access_digest.as_slice())
        .bind(expires_at)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "UPDATE commerce.order_tracking_keys SET last_used_at=$1 WHERE store_id=$2 AND id=$3",
        )
        .bind(now)
        .bind(actor.store_id.as_uuid())
        .bind(tracking_key_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let order = load_order(&mut transaction, actor, OrderId::from_uuid(order_id))
            .await?
            .ok_or_else(|| order_not_found(OrderId::from_uuid(order_id)))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(OrderTrackingSession {
            access_token,
            expires_at,
            order,
        }))
    }

    async fn get_tracked_order(
        &self,
        actor: &MachineActor,
        access_token: &SecretString,
        now: OffsetDateTime,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        if !valid_capability(access_token.expose_secret(), "ots_") {
            return Ok(None);
        }
        let mut transaction = self.begin(actor).await?;
        let digest: [u8; 32] = Sha256::digest(access_token.expose_secret()).into();
        let order_id: Option<Uuid> = sqlx::query_scalar(
            "UPDATE commerce.order_tracking_sessions AS session SET last_used_at=$1 \
             FROM commerce.order_tracking_keys AS key, commerce.orders AS order_row \
             WHERE session.store_id=$2 AND session.access_digest=$3 AND session.expires_at>$1 \
               AND key.store_id=session.store_id AND key.id=session.tracking_key_id \
               AND key.revoked_at IS NULL AND key.expires_at>$1 \
               AND order_row.store_id=key.store_id AND order_row.id=key.order_id \
               AND order_row.sales_channel_id=$4 RETURNING key.order_id",
        )
        .bind(now)
        .bind(actor.store_id.as_uuid())
        .bind(digest.as_slice())
        .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let order = match order_id {
            Some(order_id) => {
                load_order(&mut transaction, actor, OrderId::from_uuid(order_id)).await?
            }
            None => None,
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(order)
    }
}

fn valid_capability(value: &str, prefix: &str) -> bool {
    value.len() == prefix.len() + 43
        && value.starts_with(prefix)
        && value[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

#[async_trait]
impl CheckoutExpiryQueue for PostgresStorefrontSalesRepository {
    async fn claim_due_checkouts(
        &self,
        worker_id: Uuid,
        limit: u16,
        now: OffsetDateTime,
        stale_before: OffsetDateTime,
    ) -> Result<Vec<CheckoutExpiryJob>, ApplicationError> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>)>(
            "SELECT id, store_id, inventory_reservation_id \
             FROM commerce.claim_expired_checkouts($1, $2, $3, $4)",
        )
        .bind(worker_id)
        .bind(i32::from(limit))
        .bind(now)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(rows
            .into_iter()
            .map(
                |(id, store_id, inventory_reservation_id)| CheckoutExpiryJob {
                    id: CheckoutId::from_uuid(id),
                    store_id,
                    inventory_reservation_id: inventory_reservation_id
                        .map(InventoryReservationId::from_uuid),
                },
            )
            .collect())
    }

    async fn expire_checkout(
        &self,
        worker_id: Uuid,
        job: CheckoutExpiryJob,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(job.store_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        let checkout = sqlx::query_as::<_, (Option<Uuid>, String, OffsetDateTime)>(
            "SELECT inventory_reservation_id, status::text, expires_at \
             FROM commerce.checkouts \
             WHERE store_id = $1 AND id = $2 \
               AND expiry_locked_by = $3 FOR UPDATE",
        )
        .bind(job.store_id)
        .bind(job.id.as_uuid())
        .bind(worker_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(checkout_expiry_lease_lost)?;
        if checkout.1 != "pending" || checkout.2 > now {
            return Err(checkout_expiry_lease_lost());
        }
        if checkout.0
            != job
                .inventory_reservation_id
                .map(InventoryReservationId::as_uuid)
        {
            return Err(checkout_expiry_lease_lost());
        }
        if let Some(reservation_id) = job.inventory_reservation_id {
            let reservation_status: Option<String> = sqlx::query_scalar(
                "SELECT status::text FROM commerce.inventory_reservations \
                 WHERE store_id = $1 AND id = $2 FOR UPDATE",
            )
            .bind(job.store_id)
            .bind(reservation_id.as_uuid())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?;
            if reservation_status.as_deref() == Some("active") {
                close_reservation(
                    &mut transaction,
                    StoreId::from_uuid(job.store_id),
                    reservation_id,
                    ReservationClosure::Expired,
                    now,
                )
                .await?;
            }
        }
        let result = sqlx::query(
            "UPDATE commerce.checkouts \
             SET status = 'expired', closed_at = $1, updated_at = $2, \
                 expiry_locked_by = NULL, expiry_locked_at = NULL \
             WHERE store_id = $3 AND id = $4 \
               AND expiry_locked_by = $5 AND status = 'pending'",
        )
        .bind(now)
        .bind(now)
        .bind(job.store_id)
        .bind(job.id.as_uuid())
        .bind(worker_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(checkout_expiry_lease_lost());
        }
        transaction.commit().await.map_err(database_error)
    }
}

async fn select_price_list(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    channel_id: SalesChannelId,
    currency: Option<CurrencyCode>,
) -> Result<Option<(Uuid, CurrencyCode)>, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT price_list.id, price_list.currency::text \
         FROM commerce.price_lists AS price_list \
         INNER JOIN commerce.stores AS store \
           ON store.id = price_list.store_id \
         INNER JOIN commerce.sales_channels AS channel \
           ON channel.store_id = store.id AND channel.id = $1 \
         INNER JOIN commerce.store_currencies AS store_currency \
           ON store_currency.store_id = store.id \
          AND store_currency.currency = price_list.currency \
         WHERE price_list.store_id = $2 \
           AND store.status = 'active' AND channel.status = 'active' \
           AND price_list.status = 'active' AND store_currency.enabled \
           AND price_list.currency = COALESCE($3::char(3), store.default_currency) \
           AND (price_list.starts_at IS NULL OR price_list.starts_at <= CURRENT_TIMESTAMP) \
           AND (price_list.ends_at IS NULL OR price_list.ends_at > CURRENT_TIMESTAMP) \
         ORDER BY price_list.starts_at DESC NULLS LAST, price_list.id ASC LIMIT 1",
    )
    .bind(channel_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(currency.map(|value| value.as_str().to_owned()))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|(id, currency)| Ok((id, parse_currency(&currency)?)))
        .transpose()
}

async fn select_locale(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    requested: Option<Locale>,
) -> Result<Locale, ApplicationError> {
    let default: Option<String> =
        sqlx::query_scalar("SELECT default_locale FROM commerce.stores WHERE id=$1")
            .bind(actor.store_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(database_error)?;
    let default = default.ok_or_else(price_context_unavailable)?;
    let selected = requested.unwrap_or(parse_locale(&default)?);
    if selected.as_str() == default {
        return Ok(selected);
    }
    let enabled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM commerce.store_locales WHERE store_id=$1 AND locale=$2)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(selected.as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if enabled {
        Ok(selected)
    } else {
        Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "locale",
                reason: "must be enabled for the Store".into(),
            }],
        })
    }
}

async fn translation_locales(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    locale: &Locale,
) -> Result<(Option<String>, Option<String>), ApplicationError> {
    let default: String =
        sqlx::query_scalar("SELECT default_locale FROM commerce.stores WHERE id=$1")
            .bind(actor.store_id.as_uuid())
            .fetch_one(&mut **transaction)
            .await
            .map_err(database_error)?;
    if locale.as_str() == default {
        return Ok((None, None));
    }
    let language = locale.language();
    let primary = if language != locale.as_str() && language != default {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM commerce.store_locales WHERE store_id=$1 AND locale=$2)",
        )
        .bind(actor.store_id.as_uuid())
        .bind(language)
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?
        .then(|| language.to_owned())
    } else {
        None
    };
    Ok((Some(locale.as_str().to_owned()), primary))
}

async fn resolve_variant(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    channel_id: SalesChannelId,
    price_list_id: PriceListId,
    variant_id: ProductVariantId,
    locale: &Locale,
) -> Result<Option<(Uuid, String, String, Option<String>, bool, bool, i64, bool)>, ApplicationError>
{
    let translations = translation_locales(transaction, actor, locale).await?;
    sqlx::query_as(
        "SELECT product.id, COALESCE((SELECT translation.title FROM commerce.product_translations AS translation WHERE translation.store_id=product.store_id AND translation.product_id=product.id AND (translation.locale=$1 OR translation.locale=$2) ORDER BY CASE WHEN translation.locale=$3 THEN 0 ELSE 1 END LIMIT 1),product.title), COALESCE((SELECT translation.title FROM commerce.product_variant_translations AS translation WHERE translation.store_id=variant.store_id AND translation.product_id=variant.product_id AND translation.product_variant_id=variant.id AND (translation.locale=$4 OR translation.locale=$5) ORDER BY CASE WHEN translation.locale=$6 THEN 0 ELSE 1 END LIMIT 1),variant.title), variant.sku::text, \
                variant.requires_shipping, variant.track_inventory, price.amount_minor, \
                price_list.tax_inclusive \
         FROM commerce.product_variants AS variant \
         INNER JOIN commerce.products AS product \
           ON product.store_id = variant.store_id AND product.id = variant.product_id \
         INNER JOIN commerce.product_publications AS publication \
           ON publication.store_id = product.store_id AND publication.product_id = product.id \
          AND publication.sales_channel_id = $7 \
         INNER JOIN commerce.price_lists AS price_list \
           ON price_list.store_id = variant.store_id AND price_list.id = $8 \
         INNER JOIN commerce.prices AS price \
           ON price.store_id = variant.store_id AND price.price_list_id = price_list.id \
          AND price.product_variant_id = variant.id \
         WHERE variant.store_id = $9 AND variant.id = $10 \
           AND variant.status = 'active' AND product.status = 'active' \
           AND price_list.status = 'active' \
           AND (price_list.starts_at IS NULL OR price_list.starts_at <= CURRENT_TIMESTAMP) \
           AND (price_list.ends_at IS NULL OR price_list.ends_at > CURRENT_TIMESTAMP)",
    )
    .bind(translations.0.as_deref())
    .bind(translations.1.as_deref())
    .bind(translations.0.as_deref())
    .bind(translations.0.as_deref())
    .bind(translations.1.as_deref())
    .bind(translations.0.as_deref())
    .bind(channel_id.as_uuid())
    .bind(price_list_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(variant_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)
}

async fn insert_or_replace_line(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
    line: &CartLine,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.cart_lines \
         (store_id, cart_id, product_id, product_variant_id, \
          product_title, variant_title, sku, requires_shipping, track_inventory, quantity, \
          unit_price_amount_minor, tax_inclusive) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         ON CONFLICT (store_id, cart_id, product_variant_id) \
         DO UPDATE SET product_title = EXCLUDED.product_title, \
             variant_title = EXCLUDED.variant_title, sku = EXCLUDED.sku, \
             requires_shipping = EXCLUDED.requires_shipping, \
             track_inventory = EXCLUDED.track_inventory, quantity = EXCLUDED.quantity, \
             unit_price_amount_minor = EXCLUDED.unit_price_amount_minor, \
             tax_inclusive = EXCLUDED.tax_inclusive, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(actor.store_id.as_uuid())
    .bind(cart_id.as_uuid())
    .bind(line.product_id().as_uuid())
    .bind(line.product_variant_id().as_uuid())
    .bind(line.product_title())
    .bind(line.variant_title())
    .bind(line.sku())
    .bind(line.requires_shipping())
    .bind(line.track_inventory())
    .bind(i32::try_from(line.quantity()).map_err(unexpected_conversion)?)
    .bind(line.unit_price().amount_minor())
    .bind(line.tax_inclusive())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn bump_cart(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE commerce.carts SET version = version + 1, updated_at = CURRENT_TIMESTAMP \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(cart_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn lock_active_cart(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<(Uuid, Uuid, String, String), ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, String)>(
        "SELECT sales_channel_id, price_list_id, currency::text, locale, status::text \
         FROM commerce.carts WHERE store_id = $1 \
           AND sales_channel_id = $2 AND id = $3 FOR UPDATE",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| cart_not_found(cart_id))?;
    if row.4 != "active" {
        return Err(cart_not_active());
    }
    Ok((row.0, row.1, row.2, row.3))
}

async fn load_cart(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<Option<CartDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, CartHeaderRow>(
        "SELECT id, shopper_id, price_list_id, currency::text, locale, status::text, version, created_at, updated_at \
         FROM commerce.carts WHERE store_id = $1 \
           AND sales_channel_id = $2 AND id = $3",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let currency = parse_currency(&row.3)?;
    let locale = parse_locale(&row.4)?;
    let status = CartStatus::parse(&row.5).ok_or_else(corrupt_sales_state)?;
    let lines = load_cart_line_rows(transaction, actor, cart_id).await?;
    let media = load_cart_media(transaction, actor, &locale, &lines).await?;
    let items = lines
        .into_iter()
        .map(|line| cart_line_item(line, currency, &media))
        .collect::<Result<Vec<_>, _>>()?;
    let subtotal = items
        .iter()
        .try_fold(Money::zero(currency), |total, line| {
            total.checked_add(&Money::new(line.subtotal_amount_minor, currency))
        })?;
    Ok(Some(CartDetail {
        id: CartId::from_uuid(row.0),
        shopper_id: ShopperId::from_uuid(row.1),
        price_list_id: PriceListId::from_uuid(row.2),
        currency,
        locale,
        status,
        version: u64::try_from(row.6).map_err(unexpected_conversion)?,
        lines: items,
        subtotal_amount_minor: subtotal.amount_minor(),
        created_at: row.7,
        updated_at: row.8,
    }))
}

async fn load_cart_media(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    locale: &Locale,
    lines: &[CartLineRow],
) -> Result<HashMap<Uuid, Vec<StorefrontMediaAsset>>, ApplicationError> {
    let product_ids = lines.iter().map(|line| line.0).collect::<Vec<_>>();
    if product_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let (exact_locale, language_locale) = translation_locales(transaction, actor, locale).await?;
    let rows = sqlx::query_as::<_, CartMediaRow>(
        "SELECT media.product_id, media.id, media.product_variant_id, media.media_type, \
                media.media_kind::text, \
                COALESCE(\
                    (SELECT translation.alt_text FROM commerce.media_asset_translations AS translation \
                     WHERE translation.store_id = media.store_id AND translation.product_id = media.product_id \
                       AND translation.media_asset_id = media.id AND translation.locale = $3 LIMIT 1), \
                    (SELECT translation.alt_text FROM commerce.media_asset_translations AS translation \
                     WHERE translation.store_id = media.store_id AND translation.product_id = media.product_id \
                       AND translation.media_asset_id = media.id AND translation.locale = $4 LIMIT 1), \
                    media.alt_text), \
                media.position, media.public_url \
         FROM commerce.media_assets AS media \
         WHERE media.store_id = $1 AND media.product_id = ANY($2) AND media.status = 'ready' \
         ORDER BY media.product_id, media.position, media.id",
    )
    .bind(actor.store_id.as_uuid())
    .bind(&product_ids)
    .bind(exact_locale.as_deref())
    .bind(language_locale.as_deref())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let mut media = HashMap::new();
    for row in rows {
        let kind = match row.4.as_str() {
            "image" => chaos_domain::catalog::MediaKind::Image,
            "video" => chaos_domain::catalog::MediaKind::Video,
            _ => return Err(corrupt_sales_state()),
        };
        media
            .entry(row.0)
            .or_insert_with(Vec::new)
            .push(StorefrontMediaAsset {
                id: chaos_domain::catalog::MediaAssetId::from_uuid(row.1),
                product_variant_id: row
                    .2
                    .map(chaos_domain::catalog::ProductVariantId::from_uuid),
                media_type: row.3,
                kind,
                alt_text: row.5,
                position: u16::try_from(row.6).map_err(unexpected_conversion)?,
                url: row.7,
            });
    }
    Ok(media)
}

async fn load_cart_line_rows(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<Vec<CartLineRow>, ApplicationError> {
    sqlx::query_as(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor, \
                tax_inclusive FROM commerce.cart_lines \
         WHERE store_id = $1 AND cart_id = $2 \
         ORDER BY product_variant_id ASC",
    )
    .bind(actor.store_id.as_uuid())
    .bind(cart_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)
}

fn cart_line_item(
    row: CartLineRow,
    currency: CurrencyCode,
    media: &HashMap<Uuid, Vec<StorefrontMediaAsset>>,
) -> Result<CartLineItem, ApplicationError> {
    let quantity = u32::try_from(row.7).map_err(unexpected_conversion)?;
    let subtotal = Money::new(row.8, currency).checked_mul(u64::from(quantity))?;
    Ok(CartLineItem {
        product_id: ProductId::from_uuid(row.0),
        product_variant_id: ProductVariantId::from_uuid(row.1),
        product_title: row.2,
        variant_title: row.3,
        sku: row.4,
        requires_shipping: row.5,
        track_inventory: row.6,
        quantity,
        unit_price_amount_minor: row.8,
        subtotal_amount_minor: subtotal.amount_minor(),
        tax_inclusive: row.9,
        media: media.get(&row.0).cloned().unwrap_or_default(),
    })
}

async fn refresh_cart_lines(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
    channel_id: SalesChannelId,
    price_list_id: PriceListId,
    currency: CurrencyCode,
) -> Result<Vec<CartLine>, ApplicationError> {
    let rows = sqlx::query_as::<_, CartLineRow>(
        "SELECT product.id, variant.id, cart_line.product_title, cart_line.variant_title, \
                cart_line.sku::text, \
                variant.requires_shipping, variant.track_inventory, cart_line.quantity, \
                price.amount_minor, price_list.tax_inclusive \
         FROM commerce.cart_lines AS cart_line \
         INNER JOIN commerce.product_variants AS variant \
           ON variant.store_id = cart_line.store_id \
          AND variant.id = cart_line.product_variant_id AND variant.status = 'active' \
         INNER JOIN commerce.products AS product \
           ON product.store_id = variant.store_id AND product.id = variant.product_id \
          AND product.status = 'active' \
         INNER JOIN commerce.product_publications AS publication \
           ON publication.store_id = product.store_id AND publication.product_id = product.id \
          AND publication.sales_channel_id = $1 \
         INNER JOIN commerce.price_lists AS price_list \
           ON price_list.store_id = cart_line.store_id AND price_list.id = $2 \
         INNER JOIN commerce.prices AS price \
           ON price.store_id = variant.store_id AND price.price_list_id = price_list.id \
          AND price.product_variant_id = variant.id \
         WHERE cart_line.store_id = $3 \
           AND cart_line.cart_id = $4 ORDER BY variant.id ASC",
    )
    .bind(channel_id.as_uuid())
    .bind(price_list_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(cart_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    rows.into_iter()
        .map(|row| {
            CartLine::new(
                ProductId::from_uuid(row.0),
                ProductVariantId::from_uuid(row.1),
                row.2,
                row.3,
                row.4,
                row.5,
                row.6,
                u32::try_from(row.7).map_err(unexpected_conversion)?,
                Money::new(row.8, currency),
                row.9,
            )
            .map_err(ApplicationError::from)
        })
        .collect()
}

async fn require_price_list_active(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    price_list_id: PriceListId,
    currency: CurrencyCode,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.price_lists \
         WHERE store_id = $1 AND id = $2 \
           AND currency = $3 AND status = 'active' \
           AND (starts_at IS NULL OR starts_at <= $4) \
           AND (ends_at IS NULL OR ends_at > $5))",
    )
    .bind(actor.store_id.as_uuid())
    .bind(price_list_id.as_uuid())
    .bind(currency.as_str())
    .bind(now)
    .bind(now)
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if active {
        Ok(())
    } else {
        Err(price_context_unavailable())
    }
}

async fn reserve_inventory(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    channel_id: SalesChannelId,
    cart: &Cart,
    expires_at: OffsetDateTime,
) -> Result<Option<InventoryReservationId>, ApplicationError> {
    if !cart.lines().iter().any(CartLine::track_inventory) {
        return Ok(None);
    }
    let reservation_id = InventoryReservationId::new();
    sqlx::query(
        "INSERT INTO commerce.inventory_reservations \
         (id, store_id, sales_channel_id, expires_at) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(reservation_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(expires_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    for line in cart.lines().iter().filter(|line| line.track_inventory()) {
        let stocks = sqlx::query_as::<_, (Uuid, i64, i64)>(
            "SELECT stock.id, stock.on_hand_quantity, stock.reserved_quantity \
             FROM commerce.stock_items AS stock \
             INNER JOIN commerce.inventory_locations AS location \
               ON location.store_id = stock.store_id AND location.id = stock.inventory_location_id \
             WHERE stock.store_id = $1 \
               AND stock.product_variant_id = $2 AND location.status = 'active' \
               AND stock.on_hand_quantity > stock.reserved_quantity \
             ORDER BY stock.id ASC FOR UPDATE OF stock",
        )
        .bind(actor.store_id.as_uuid())
        .bind(line.product_variant_id().as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let mut remaining = i64::from(line.quantity());
        for (stock_item_id, on_hand, reserved) in stocks {
            if remaining == 0 {
                break;
            }
            let current = StockBalance::new(on_hand, reserved)?;
            let allocated = remaining.min(current.available());
            if allocated == 0 {
                continue;
            }
            let balance = current.reserve(allocated)?;
            sqlx::query(
                "UPDATE commerce.stock_items SET reserved_quantity = $1, \
                        updated_at = CURRENT_TIMESTAMP WHERE id = $2",
            )
            .bind(balance.reserved())
            .bind(stock_item_id)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO commerce.inventory_reservation_lines \
                 (store_id, reservation_id, stock_item_id, \
                  product_variant_id, quantity) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(actor.store_id.as_uuid())
            .bind(reservation_id.as_uuid())
            .bind(stock_item_id)
            .bind(line.product_variant_id().as_uuid())
            .bind(allocated)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO commerce.stock_ledger_entries \
                 (id, store_id, stock_item_id, reservation_id, kind, \
                  on_hand_delta_quantity, reserved_delta_quantity, resulting_on_hand_quantity, \
                  resulting_reserved_quantity) \
                 VALUES ($1, $2, $3, $4, 'reservation_created', 0, $5, $6, $7)",
            )
            .bind(Uuid::now_v7())
            .bind(actor.store_id.as_uuid())
            .bind(stock_item_id)
            .bind(reservation_id.as_uuid())
            .bind(allocated)
            .bind(balance.on_hand())
            .bind(balance.reserved())
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            remaining -= allocated;
        }
        if remaining != 0 {
            return Err(insufficient_inventory(line.product_variant_id()));
        }
    }
    Ok(Some(reservation_id))
}

async fn insert_checkout(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    shopper_id: ShopperId,
    channel_id: SalesChannelId,
    checkout: &Checkout,
    locale: &Locale,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.checkouts \
         (id, store_id, cart_id, shopper_id, sales_channel_id, price_list_id, \
          inventory_reservation_id, currency, locale, subtotal_amount_minor, discount_amount_minor, \
          tax_amount_minor, tax_inclusive, shipping_amount_minor, total_amount_minor, expires_at) \
         VALUES ($1, $2, $3, $4, \
                 $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(checkout.id().as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(checkout.cart_id().as_uuid())
    .bind(shopper_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(checkout.price_list_id().as_uuid())
    .bind(
        checkout
            .reservation_id()
            .map(InventoryReservationId::as_uuid),
    )
    .bind(checkout.currency().as_str())
    .bind(locale.as_str())
    .bind(checkout.subtotal().amount_minor())
    .bind(checkout.discount().amount_minor())
    .bind(checkout.tax().amount_minor())
    .bind(checkout.tax_inclusive())
    .bind(
        checkout
            .shipping()
            .map_or(0, |selection| selection.amount().amount_minor()),
    )
    .bind(checkout.total().amount_minor())
    .bind(checkout.expires_at())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    insert_checkout_identity(transaction, actor, checkout).await?;
    insert_checkout_tax(transaction, actor, checkout).await?;
    if checkout.promotion().is_some() {
        insert_checkout_promotion(transaction, actor, checkout).await?;
    }
    if let Some(selection) = checkout.shipping() {
        insert_checkout_shipping(transaction, actor, checkout.id(), selection).await?;
    }
    for (position, line) in checkout.lines().iter().enumerate() {
        let cart_line = line.cart_line();
        sqlx::query(
            "INSERT INTO commerce.checkout_lines \
             (store_id, checkout_id, position, product_id, \
              product_variant_id, product_title, variant_title, sku, requires_shipping, \
              track_inventory, quantity, unit_price_amount_minor, subtotal_amount_minor, \
              discount_amount_minor, tax_amount_minor, total_amount_minor, tax_inclusive) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, \
                     $13, $14, $15, $16, $17)",
        )
        .bind(actor.store_id.as_uuid())
        .bind(checkout.id().as_uuid())
        .bind(i16::try_from(position).map_err(unexpected_conversion)?)
        .bind(cart_line.product_id().as_uuid())
        .bind(cart_line.product_variant_id().as_uuid())
        .bind(cart_line.product_title())
        .bind(cart_line.variant_title())
        .bind(cart_line.sku())
        .bind(cart_line.requires_shipping())
        .bind(cart_line.track_inventory())
        .bind(i32::try_from(cart_line.quantity()).map_err(unexpected_conversion)?)
        .bind(cart_line.unit_price().amount_minor())
        .bind(line.subtotal().amount_minor())
        .bind(line.discount().amount_minor())
        .bind(line.tax().amount_minor())
        .bind(line.total().amount_minor())
        .bind(cart_line.tax_inclusive())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

async fn insert_checkout_identity(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout: &Checkout,
) -> Result<(), ApplicationError> {
    let identity = checkout.identity();
    sqlx::query(
        "INSERT INTO commerce.checkout_contacts \
         (store_id, checkout_id, email, phone) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout.id().as_uuid())
    .bind(identity.contact().email())
    .bind(identity.contact().phone())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    insert_checkout_address(
        transaction,
        actor,
        checkout.id(),
        "billing",
        identity.billing_address(),
    )
    .await?;
    if let Some(address) = identity.shipping_address() {
        insert_checkout_address(transaction, actor, checkout.id(), "shipping", address).await?;
    }
    Ok(())
}

async fn insert_checkout_shipping(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
    selection: &ShippingSelection,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.checkout_shipping_selections \
         (store_id, checkout_id, shipping_service_id, service_code, \
          service_name, amount_minor, currency, estimated_min_days, estimated_max_days) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .bind(selection.service_id().as_uuid())
    .bind(selection.code())
    .bind(selection.name())
    .bind(selection.amount().amount_minor())
    .bind(selection.amount().currency().as_str())
    .bind(i16::try_from(selection.estimated_min_days()).map_err(unexpected_conversion)?)
    .bind(i16::try_from(selection.estimated_max_days()).map_err(unexpected_conversion)?)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn insert_checkout_tax(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout: &Checkout,
) -> Result<(), ApplicationError> {
    let rule = checkout.tax_rule();
    sqlx::query(
        "INSERT INTO commerce.checkout_tax_calculations \
         (store_id, checkout_id, tax_rule_id, rule_code, rule_name, \
          country_code, rate_basis_points) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout.id().as_uuid())
    .bind(rule.rule_id().as_uuid())
    .bind(rule.code())
    .bind(rule.name())
    .bind(rule.country_code())
    .bind(i32::try_from(rule.rate_basis_points()).map_err(unexpected_conversion)?)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn insert_checkout_promotion(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout: &Checkout,
) -> Result<(), ApplicationError> {
    let promotion = checkout.promotion().ok_or_else(corrupt_sales_state)?;
    sqlx::query(
        "INSERT INTO commerce.checkout_promotion_calculations \
         (store_id, checkout_id, promotion_id, handle, name, trigger, \
          redemption_code, value_kind, rate_basis_points, amount_minor, maximum_amount_minor, \
          currency, minimum_subtotal_amount_minor, priority, starts_at, ends_at, \
          discount_amount_minor) \
         VALUES ($1,$2,$3,$4,$5,$6::commerce.promotion_trigger,$7, \
                 $8::commerce.promotion_value_kind,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout.id().as_uuid())
    .bind(promotion.promotion_id().as_uuid())
    .bind(promotion.handle())
    .bind(promotion.name())
    .bind(promotion.trigger().as_str())
    .bind(promotion.redemption_code())
    .bind(promotion.value().kind())
    .bind(
        promotion
            .value()
            .rate_basis_points()
            .map(i32::try_from)
            .transpose()
            .map_err(unexpected_conversion)?,
    )
    .bind(promotion.value().amount_minor())
    .bind(promotion.value().maximum_amount_minor())
    .bind(promotion.currency().as_str())
    .bind(promotion.minimum_subtotal_amount_minor())
    .bind(i16::try_from(promotion.priority()).map_err(unexpected_conversion)?)
    .bind(promotion.starts_at())
    .bind(promotion.ends_at())
    .bind(checkout.discount().amount_minor())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn insert_checkout_address(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
    kind: &'static str,
    address: &PostalAddress,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.checkout_addresses \
         (store_id, checkout_id, kind, full_name, company, address_line1, \
          address_line2, locality, administrative_area, postal_code, country_code) \
         VALUES ($1, $2, $3::commerce.address_kind, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .bind(kind)
    .bind(address.full_name())
    .bind(address.company())
    .bind(address.address_line1())
    .bind(address.address_line2())
    .bind(address.locality())
    .bind(address.administrative_area())
    .bind(address.postal_code())
    .bind(address.country_code())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn copy_checkout_identity_to_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    let contact = sqlx::query(
        "INSERT INTO commerce.order_contacts \
         (store_id, order_id, email, phone) \
         SELECT store_id, $1, email, phone \
         FROM commerce.checkout_contacts \
         WHERE store_id = $2 AND checkout_id = $3",
    )
    .bind(order_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    let addresses = sqlx::query(
        "INSERT INTO commerce.order_addresses \
         (store_id, order_id, kind, full_name, company, address_line1, \
          address_line2, locality, administrative_area, postal_code, country_code) \
         SELECT store_id, $1, kind, full_name, company, address_line1, \
                address_line2, locality, administrative_area, postal_code, country_code \
         FROM commerce.checkout_addresses \
         WHERE store_id = $2 AND checkout_id = $3",
    )
    .bind(order_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if contact.rows_affected() != 1 || addresses.rows_affected() == 0 {
        return Err(corrupt_sales_state());
    }
    Ok(())
}

async fn copy_checkout_shipping_to_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.order_shipping_selections \
         (store_id, order_id, shipping_service_id, service_code, service_name, \
          amount_minor, currency, estimated_min_days, estimated_max_days) \
         SELECT store_id, $1, shipping_service_id, service_code, service_name, \
                amount_minor, currency, estimated_min_days, estimated_max_days \
         FROM commerce.checkout_shipping_selections \
         WHERE store_id = $2 AND checkout_id = $3",
    )
    .bind(order_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn copy_checkout_tax_to_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "INSERT INTO commerce.order_tax_calculations \
         (store_id, order_id, tax_rule_id, rule_code, rule_name, \
          country_code, rate_basis_points) \
         SELECT store_id, $1, tax_rule_id, rule_code, rule_name, \
                country_code, rate_basis_points FROM commerce.checkout_tax_calculations \
         WHERE store_id = $2 AND checkout_id = $3",
    )
    .bind(order_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(corrupt_sales_state());
    }
    Ok(())
}

async fn copy_checkout_promotion_to_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.order_promotion_calculations \
         (store_id, order_id, promotion_id, handle, name, trigger, \
          redemption_code, value_kind, rate_basis_points, amount_minor, maximum_amount_minor, \
          currency, minimum_subtotal_amount_minor, priority, starts_at, ends_at, \
          discount_amount_minor) \
         SELECT store_id, $1, promotion_id, handle, name, trigger, \
                redemption_code, value_kind, rate_basis_points, amount_minor, maximum_amount_minor, \
                currency, minimum_subtotal_amount_minor, priority, starts_at, ends_at, \
                discount_amount_minor \
         FROM commerce.checkout_promotion_calculations \
         WHERE store_id = $2 AND checkout_id = $3",
    )
    .bind(order_id.as_uuid()).bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .execute(&mut **transaction).await.map_err(database_error)?;
    Ok(())
}

async fn load_checkout_identity(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
) -> Result<CheckoutIdentity, ApplicationError> {
    let contact = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT email::text, phone FROM commerce.checkout_contacts \
         WHERE store_id = $1 AND checkout_id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_sales_state)?;
    let addresses = sqlx::query_as::<_, AddressRow>(
        "SELECT kind::text, full_name, company, address_line1, address_line2, locality, \
                administrative_area, postal_code, country_code::text \
         FROM commerce.checkout_addresses \
         WHERE store_id = $1 AND checkout_id = $2 ORDER BY kind",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    build_identity(contact, addresses)
}

async fn load_order_identity(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<CheckoutIdentity, ApplicationError> {
    let contact = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT email::text, phone FROM commerce.order_contacts \
         WHERE store_id = $1 AND order_id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_sales_state)?;
    let addresses = sqlx::query_as::<_, AddressRow>(
        "SELECT kind::text, full_name, company, address_line1, address_line2, locality, \
                administrative_area, postal_code, country_code::text \
         FROM commerce.order_addresses \
         WHERE store_id = $1 AND order_id = $2 ORDER BY kind",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    build_identity(contact, addresses)
}

fn build_identity(
    contact: (String, Option<String>),
    addresses: Vec<AddressRow>,
) -> Result<CheckoutIdentity, ApplicationError> {
    let contact = CheckoutContact::new(contact.0, contact.1)?;
    let mut billing = None;
    let mut shipping = None;
    for address in addresses {
        let kind = address.0.clone();
        let address = postal_address_from_row(address)?;
        match kind.as_str() {
            "billing" if billing.is_none() => billing = Some(address),
            "shipping" if shipping.is_none() => shipping = Some(address),
            _ => return Err(corrupt_sales_state()),
        }
    }
    Ok(CheckoutIdentity::new(
        contact,
        billing.ok_or_else(corrupt_sales_state)?,
        shipping,
    ))
}

fn postal_address_from_row(row: AddressRow) -> Result<PostalAddress, ApplicationError> {
    PostalAddress::new(row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8)
        .map_err(ApplicationError::from)
}

async fn load_active_tax_rule(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    country_code: &str,
) -> Result<TaxRule, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String, i32)>(
        "SELECT id, code, name, country_code::text, rate_basis_points \
         FROM commerce.tax_rules \
         WHERE store_id = $1 \
           AND country_code = $2 AND status = 'active' FOR SHARE",
    )
    .bind(actor.store_id.as_uuid())
    .bind(country_code)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| tax_rule_unavailable(country_code))?;
    TaxRule::rehydrate(
        TaxRuleId::from_uuid(row.0),
        row.1,
        row.2,
        row.3,
        u32::try_from(row.4).map_err(unexpected_conversion)?,
        TaxRuleStatus::Active,
    )
    .map_err(ApplicationError::from)
}

async fn select_promotion(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    currency: CurrencyCode,
    subtotals: &[Money],
    requested_code: Option<&str>,
    now: OffsetDateTime,
) -> Result<Option<(Promotion, Vec<Money>)>, ApplicationError> {
    let requested_code = requested_code.map(|code| code.trim().to_ascii_uppercase());
    if requested_code.as_deref().is_some_and(str::is_empty) {
        return Err(invalid_promotion_code());
    }
    let rows = sqlx::query_as::<_, PromotionCheckoutRow>(
        "SELECT id, handle, name, trigger::text, redemption_code::text, value_kind::text, \
                rate_basis_points, amount_minor, maximum_amount_minor, \
                minimum_subtotal_amount_minor, priority, starts_at, ends_at \
         FROM commerce.promotions \
         WHERE store_id = $1 AND currency = $2 \
           AND status = 'active' \
           AND (trigger = 'automatic' OR (trigger = 'code' AND redemption_code = $3)) \
         ORDER BY priority, id FOR SHARE",
    )
    .bind(actor.store_id.as_uuid())
    .bind(currency.as_str())
    .bind(requested_code.as_deref())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;

    let mut requested_code_eligible = requested_code.is_none();
    let mut best: Option<(Promotion, Vec<Money>, i64)> = None;
    for row in rows {
        let value = match row.value_kind.as_str() {
            "percentage" => PromotionValue::Percentage {
                rate_basis_points: u32::try_from(
                    row.rate_basis_points.ok_or_else(corrupt_sales_state)?,
                )
                .map_err(unexpected_conversion)?,
                maximum_amount_minor: row.maximum_amount_minor,
            },
            "fixed_amount" => PromotionValue::FixedAmount {
                amount_minor: row.amount_minor.ok_or_else(corrupt_sales_state)?,
            },
            _ => return Err(corrupt_sales_state()),
        };
        let promotion = Promotion::rehydrate(
            PromotionId::from_uuid(row.id),
            row.handle,
            row.name,
            PromotionTrigger::parse(&row.trigger).ok_or_else(corrupt_sales_state)?,
            row.redemption_code,
            value,
            currency,
            row.minimum_subtotal_amount_minor,
            u16::try_from(row.priority).map_err(unexpected_conversion)?,
            row.starts_at,
            row.ends_at,
            PromotionStatus::Active,
        )?;
        let Some(allocations) = promotion.calculate_and_allocate(subtotals, now)? else {
            continue;
        };
        if promotion.trigger() == PromotionTrigger::Code {
            requested_code_eligible = true;
        }
        let amount = allocations.iter().try_fold(0_i64, |total, allocation| {
            total
                .checked_add(allocation.amount_minor())
                .ok_or_else(corrupt_sales_state)
        })?;
        let replace = best.as_ref().is_none_or(|(current, _, current_amount)| {
            amount > *current_amount
                || (amount == *current_amount
                    && (promotion.priority(), promotion.id()) < (current.priority(), current.id()))
        });
        if replace {
            best = Some((promotion, allocations, amount));
        }
    }
    if !requested_code_eligible {
        return Err(invalid_promotion_code());
    }
    Ok(best.map(|(promotion, allocations, _)| (promotion, allocations)))
}

async fn load_active_shipping_service(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    service_id: ShippingServiceId,
    currency: CurrencyCode,
    country_code: &str,
) -> Result<ShippingSelection, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, i64, String, i16, i16)>(
        "SELECT s.id, s.code, s.name, s.amount_minor, s.currency::text, \
                s.estimated_min_days, s.estimated_max_days \
         FROM commerce.shipping_services s \
         JOIN commerce.shipping_service_regions r \
           ON r.store_id = s.store_id \
          AND r.shipping_service_id = s.id \
         WHERE s.store_id = $1 AND s.id = $2 \
           AND s.currency = $3 AND s.status = 'active' AND r.country_code = $4 \
         FOR SHARE OF s",
    )
    .bind(actor.store_id.as_uuid())
    .bind(service_id.as_uuid())
    .bind(currency.as_str())
    .bind(country_code)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(invalid_shipping_selection)?;
    shipping_selection_from_row(row)
}

fn shipping_selection_from_row(
    row: (Uuid, String, String, i64, String, i16, i16),
) -> Result<ShippingSelection, ApplicationError> {
    let currency = parse_currency(&row.4)?;
    ShippingSelection::rehydrate(
        ShippingServiceId::from_uuid(row.0),
        row.1,
        row.2,
        Money::new(row.3, currency),
        u16::try_from(row.5).map_err(unexpected_conversion)?,
        u16::try_from(row.6).map_err(unexpected_conversion)?,
    )
    .map_err(ApplicationError::from)
}

async fn load_checkout_shipping(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
) -> Result<Option<ShippingSelection>, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, i64, String, i16, i16)>(
        "SELECT shipping_service_id, service_code, service_name, amount_minor, currency::text, \
                estimated_min_days, estimated_max_days \
         FROM commerce.checkout_shipping_selections \
         WHERE store_id = $1 AND checkout_id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(shipping_selection_from_row).transpose()
}

async fn load_order_shipping(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<Option<ShippingSelection>, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, i64, String, i16, i16)>(
        "SELECT shipping_service_id, service_code, service_name, amount_minor, currency::text, \
                estimated_min_days, estimated_max_days \
         FROM commerce.order_shipping_selections \
         WHERE store_id = $1 AND order_id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(shipping_selection_from_row).transpose()
}

fn tax_snapshot_from_row(
    row: (Uuid, String, String, String, i32),
) -> Result<TaxRuleSnapshot, ApplicationError> {
    TaxRuleSnapshot::rehydrate(
        TaxRuleId::from_uuid(row.0),
        row.1,
        row.2,
        row.3,
        u32::try_from(row.4).map_err(unexpected_conversion)?,
    )
    .map_err(ApplicationError::from)
}

async fn load_checkout_tax(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
) -> Result<TaxRuleSnapshot, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String, i32)>(
        "SELECT tax_rule_id, rule_code, rule_name, country_code::text, rate_basis_points \
         FROM commerce.checkout_tax_calculations \
         WHERE store_id = $1 AND checkout_id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_sales_state)?;
    tax_snapshot_from_row(row)
}

async fn load_order_tax(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<TaxRuleSnapshot, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String, i32)>(
        "SELECT tax_rule_id, rule_code, rule_name, country_code::text, rate_basis_points \
         FROM commerce.order_tax_calculations \
         WHERE store_id = $1 AND order_id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_sales_state)?;
    tax_snapshot_from_row(row)
}

fn promotion_snapshot_from_row(
    row: PromotionSnapshotRow,
) -> Result<PromotionSnapshot, ApplicationError> {
    let value = match row.value_kind.as_str() {
        "percentage" => PromotionValue::Percentage {
            rate_basis_points: u32::try_from(
                row.rate_basis_points.ok_or_else(corrupt_sales_state)?,
            )
            .map_err(unexpected_conversion)?,
            maximum_amount_minor: row.maximum_amount_minor,
        },
        "fixed_amount" => PromotionValue::FixedAmount {
            amount_minor: row.amount_minor.ok_or_else(corrupt_sales_state)?,
        },
        _ => return Err(corrupt_sales_state()),
    };
    PromotionSnapshot::rehydrate(
        PromotionId::from_uuid(row.promotion_id),
        row.handle,
        row.name,
        PromotionTrigger::parse(&row.trigger).ok_or_else(corrupt_sales_state)?,
        row.redemption_code,
        value,
        parse_currency(&row.currency)?,
        row.minimum_subtotal_amount_minor,
        u16::try_from(row.priority).map_err(unexpected_conversion)?,
        row.starts_at,
        row.ends_at,
    )
    .map_err(ApplicationError::from)
}

async fn load_checkout_promotion(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
) -> Result<Option<PromotionSnapshot>, ApplicationError> {
    sqlx::query_as::<_, PromotionSnapshotRow>(
        "SELECT promotion_id, handle, name, trigger::text, redemption_code, value_kind::text, \
                rate_basis_points, amount_minor, maximum_amount_minor, currency::text, \
                minimum_subtotal_amount_minor, priority, starts_at, ends_at \
         FROM commerce.checkout_promotion_calculations \
         WHERE store_id = $1 AND checkout_id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(promotion_snapshot_from_row)
    .transpose()
}

async fn load_order_promotion(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<Option<PromotionSnapshot>, ApplicationError> {
    sqlx::query_as::<_, PromotionSnapshotRow>(
        "SELECT promotion_id, handle, name, trigger::text, redemption_code, value_kind::text, \
                rate_basis_points, amount_minor, maximum_amount_minor, currency::text, \
                minimum_subtotal_amount_minor, priority, starts_at, ends_at \
         FROM commerce.order_promotion_calculations \
         WHERE store_id = $1 AND order_id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(promotion_snapshot_from_row)
    .transpose()
}

async fn load_checkout(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
) -> Result<Option<CheckoutDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, CheckoutHeaderRow>(
        "SELECT id, shopper_id, cart_id, inventory_reservation_id, price_list_id, currency::text, \
                status::text, subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                tax_inclusive, shipping_amount_minor, total_amount_minor, expires_at, created_at FROM commerce.checkouts \
         WHERE store_id = $1 AND sales_channel_id = $2 AND id = $3",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(checkout_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let locale: String =
        sqlx::query_scalar("SELECT locale FROM commerce.checkouts WHERE store_id=$1 AND id=$2")
            .bind(actor.store_id.as_uuid())
            .bind(checkout_id.as_uuid())
            .fetch_one(&mut **transaction)
            .await
            .map_err(database_error)?;
    let identity = load_checkout_identity(transaction, actor, checkout_id).await?;
    let shipping = load_checkout_shipping(transaction, actor, checkout_id).await?;
    let tax_rule = load_checkout_tax(transaction, actor, checkout_id).await?;
    let promotion = load_checkout_promotion(transaction, actor, checkout_id).await?;
    let lines = sqlx::query_as::<_, CheckoutLineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, quantity, unit_price_amount_minor, subtotal_amount_minor, \
                discount_amount_minor, tax_amount_minor, total_amount_minor, tax_inclusive \
         FROM commerce.checkout_lines WHERE store_id = $1 \
           AND checkout_id = $2 ORDER BY position ASC",
    )
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(Some(CheckoutDetail {
        id: CheckoutId::from_uuid(row.0),
        shopper_id: ShopperId::from_uuid(row.1),
        cart_id: CartId::from_uuid(row.2),
        inventory_reservation_id: row.3.map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(row.4),
        currency: parse_currency(&row.5)?,
        locale: parse_locale(&locale)?,
        status: row.6,
        identity,
        subtotal_amount_minor: row.7,
        discount_amount_minor: row.8,
        tax_amount_minor: row.9,
        tax_rule,
        promotion,
        tax_inclusive: row.10,
        shipping,
        shipping_amount_minor: row.11,
        total_amount_minor: row.12,
        expires_at: row.13,
        lines: lines
            .into_iter()
            .map(checkout_line_item)
            .collect::<Result<Vec<_>, _>>()?,
        created_at: row.14,
    }))
}

fn checkout_line_item(row: CheckoutLineRow) -> Result<CheckoutLineItem, ApplicationError> {
    Ok(CheckoutLineItem {
        product_id: ProductId::from_uuid(row.0),
        product_variant_id: ProductVariantId::from_uuid(row.1),
        product_title: row.2,
        variant_title: row.3,
        sku: row.4,
        requires_shipping: row.5,
        quantity: u32::try_from(row.6).map_err(unexpected_conversion)?,
        unit_price_amount_minor: row.7,
        subtotal_amount_minor: row.8,
        discount_amount_minor: row.9,
        tax_amount_minor: row.10,
        total_amount_minor: row.11,
        tax_inclusive: row.12,
    })
}

pub(super) async fn load_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, OrderHeaderRow>(
        "SELECT id, shopper_id, checkout_id, inventory_reservation_id, price_list_id, currency::text, \
                status::text, subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                tax_inclusive, shipping_amount_minor, total_amount_minor, created_at, updated_at FROM commerce.orders \
         WHERE store_id = $1 AND sales_channel_id = $2 AND id = $3",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let (locale, order_number): (String, String) = sqlx::query_as(
        "SELECT locale, order_number FROM commerce.orders WHERE store_id=$1 AND id=$2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    let derived_statuses = sqlx::query_as::<_, (String, String)>(
        "SELECT fulfillment_status::text, delivery_status::text FROM commerce.orders \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    let identity = load_order_identity(transaction, actor, order_id).await?;
    let shipping = load_order_shipping(transaction, actor, order_id).await?;
    let tax_rule = load_order_tax(transaction, actor, order_id).await?;
    let promotion = load_order_promotion(transaction, actor, order_id).await?;
    let lines = sqlx::query_as::<_, OrderLineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor, \
                subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                total_amount_minor, tax_inclusive FROM commerce.order_lines \
         WHERE store_id = $1 AND order_id = $2 ORDER BY position",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let transitions = sqlx::query_as::<
        _,
        (
            Uuid,
            Option<String>,
            String,
            String,
            Option<Uuid>,
            OffsetDateTime,
        ),
    >(
        "SELECT id, from_status::text, to_status::text, kind::text, actor_user_id, occurred_at \
         FROM commerce.order_transitions WHERE store_id = $1 \
           AND order_id = $2 ORDER BY occurred_at, id",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(Some(OrderDetail {
        id: OrderId::from_uuid(row.0),
        order_number: OrderNumber::parse(order_number)?,
        shopper_id: ShopperId::from_uuid(row.1),
        checkout_id: CheckoutId::from_uuid(row.2),
        inventory_reservation_id: row.3.map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(row.4),
        currency: parse_currency(&row.5)?,
        locale: parse_locale(&locale)?,
        status: OrderStatus::parse(&row.6).ok_or_else(corrupt_sales_state)?,
        fulfillment_status: OrderFulfillmentStatus::parse(&derived_statuses.0)
            .ok_or_else(corrupt_sales_state)?,
        delivery_status: OrderDeliveryStatus::parse(&derived_statuses.1)
            .ok_or_else(corrupt_sales_state)?,
        identity,
        subtotal_amount_minor: row.7,
        discount_amount_minor: row.8,
        tax_amount_minor: row.9,
        tax_rule,
        promotion,
        tax_inclusive: row.10,
        shipping,
        shipping_amount_minor: row.11,
        total_amount_minor: row.12,
        lines: lines
            .into_iter()
            .map(order_line_item)
            .collect::<Result<Vec<_>, _>>()?,
        transitions: transitions
            .into_iter()
            .map(|value| {
                Ok(OrderTransitionItem {
                    id: value.0,
                    from_status: match value.1.as_deref() {
                        Some(status) => {
                            Some(OrderStatus::parse(status).ok_or_else(corrupt_sales_state)?)
                        }
                        None => None,
                    },
                    to_status: OrderStatus::parse(&value.2).ok_or_else(corrupt_sales_state)?,
                    kind: value.3,
                    actor_user_id: value.4,
                    occurred_at: value.5,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        created_at: row.13,
        updated_at: row.14,
    }))
}

fn order_line_item(row: OrderLineRow) -> Result<OrderLineItem, ApplicationError> {
    Ok(OrderLineItem {
        product_id: ProductId::from_uuid(row.0),
        product_variant_id: ProductVariantId::from_uuid(row.1),
        product_title: row.2,
        variant_title: row.3,
        sku: row.4,
        requires_shipping: row.5,
        track_inventory: row.6,
        quantity: u32::try_from(row.7).map_err(unexpected_conversion)?,
        unit_price_amount_minor: row.8,
        subtotal_amount_minor: row.9,
        discount_amount_minor: row.10,
        tax_amount_minor: row.11,
        total_amount_minor: row.12,
        tax_inclusive: row.13,
    })
}

#[derive(Serialize, Deserialize)]
struct CartSnapshot {
    id: Uuid,
    shopper_id: Uuid,
    price_list_id: Uuid,
    currency: String,
    #[serde(default = "default_locale_snapshot")]
    locale: String,
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
    tax_inclusive: bool,
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

#[derive(Serialize, Deserialize)]
struct CheckoutSnapshot {
    id: Uuid,
    shopper_id: Uuid,
    cart_id: Uuid,
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    #[serde(default = "default_locale_snapshot")]
    locale: String,
    status: String,
    identity: CheckoutIdentitySnapshot,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    tax_rule: TaxRuleSnapshotData,
    promotion: Option<PromotionSnapshotData>,
    tax_inclusive: bool,
    shipping: Option<ShippingSelectionSnapshot>,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    expires_at: String,
    lines: Vec<CheckoutLineSnapshot>,
    created_at: String,
}

#[derive(Serialize, Deserialize)]
struct CheckoutLineSnapshot {
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    sku: Option<String>,
    requires_shipping: bool,
    quantity: u32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    tax_inclusive: bool,
}

#[derive(Serialize, Deserialize)]
struct OrderSnapshot {
    id: Uuid,
    order_number: String,
    shopper_id: Uuid,
    checkout_id: Uuid,
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    #[serde(default = "default_locale_snapshot")]
    locale: String,
    status: String,
    fulfillment_status: String,
    delivery_status: String,
    identity: CheckoutIdentitySnapshot,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    tax_rule: TaxRuleSnapshotData,
    promotion: Option<PromotionSnapshotData>,
    tax_inclusive: bool,
    shipping: Option<ShippingSelectionSnapshot>,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    lines: Vec<OrderLineSnapshot>,
    transitions: Vec<OrderTransitionSnapshot>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct OrderLineSnapshot {
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
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    total_amount_minor: i64,
    tax_inclusive: bool,
}

#[derive(Serialize, Deserialize)]
struct OrderTransitionSnapshot {
    id: Uuid,
    from_status: Option<String>,
    to_status: String,
    kind: String,
    actor_user_id: Option<Uuid>,
    occurred_at: String,
}

#[derive(Serialize, Deserialize)]
struct ShippingSelectionSnapshot {
    service_id: Uuid,
    code: String,
    name: String,
    amount_minor: i64,
    currency: String,
    estimated_min_days: u16,
    estimated_max_days: u16,
}

#[derive(Serialize, Deserialize)]
struct TaxRuleSnapshotData {
    rule_id: Uuid,
    code: String,
    name: String,
    country_code: String,
    rate_basis_points: u32,
}

#[derive(Serialize, Deserialize)]
struct PromotionSnapshotData {
    promotion_id: Uuid,
    handle: String,
    name: String,
    trigger: String,
    redemption_code: Option<String>,
    value_kind: String,
    rate_basis_points: Option<u32>,
    amount_minor: Option<i64>,
    maximum_amount_minor: Option<i64>,
    currency: String,
    minimum_subtotal_amount_minor: i64,
    priority: u16,
    starts_at: Option<String>,
    ends_at: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CheckoutIdentitySnapshot {
    contact: CheckoutContactSnapshot,
    billing_address: PostalAddressSnapshot,
    shipping_address: Option<PostalAddressSnapshot>,
}

#[derive(Serialize, Deserialize)]
struct CheckoutContactSnapshot {
    email: String,
    phone: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct PostalAddressSnapshot {
    full_name: String,
    company: Option<String>,
    address_line1: String,
    address_line2: Option<String>,
    locality: String,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: String,
}

fn cart_snapshot(detail: &CartDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(CartSnapshot {
        id: detail.id.as_uuid(),
        shopper_id: detail.shopper_id.as_uuid(),
        price_list_id: detail.price_list_id.as_uuid(),
        currency: detail.currency.as_str().into(),
        locale: detail.locale.as_str().into(),
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
        locale: parse_locale(&snapshot.locale)?,
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

fn checkout_snapshot(detail: &CheckoutDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(CheckoutSnapshot {
        id: detail.id.as_uuid(),
        shopper_id: detail.shopper_id.as_uuid(),
        cart_id: detail.cart_id.as_uuid(),
        inventory_reservation_id: detail
            .inventory_reservation_id
            .map(InventoryReservationId::as_uuid),
        price_list_id: detail.price_list_id.as_uuid(),
        currency: detail.currency.as_str().into(),
        locale: detail.locale.as_str().into(),
        status: detail.status.clone(),
        identity: CheckoutIdentitySnapshot::from(&detail.identity),
        subtotal_amount_minor: detail.subtotal_amount_minor,
        discount_amount_minor: detail.discount_amount_minor,
        tax_amount_minor: detail.tax_amount_minor,
        tax_rule: TaxRuleSnapshotData::from(&detail.tax_rule),
        promotion: detail
            .promotion
            .as_ref()
            .map(PromotionSnapshotData::try_from)
            .transpose()?,
        tax_inclusive: detail.tax_inclusive,
        shipping: detail
            .shipping
            .as_ref()
            .map(ShippingSelectionSnapshot::from),
        shipping_amount_minor: detail.shipping_amount_minor,
        total_amount_minor: detail.total_amount_minor,
        expires_at: format_time(detail.expires_at)?,
        lines: detail
            .lines
            .iter()
            .map(CheckoutLineSnapshot::from)
            .collect(),
        created_at: format_time(detail.created_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_checkout(value: Value) -> Result<CheckoutDetail, ApplicationError> {
    let snapshot: CheckoutSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(CheckoutDetail {
        id: CheckoutId::from_uuid(snapshot.id),
        shopper_id: ShopperId::from_uuid(snapshot.shopper_id),
        cart_id: CartId::from_uuid(snapshot.cart_id),
        inventory_reservation_id: snapshot
            .inventory_reservation_id
            .map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(snapshot.price_list_id),
        currency: parse_currency(&snapshot.currency)?,
        locale: parse_locale(&snapshot.locale)?,
        status: snapshot.status,
        identity: snapshot.identity.try_into()?,
        subtotal_amount_minor: snapshot.subtotal_amount_minor,
        discount_amount_minor: snapshot.discount_amount_minor,
        tax_amount_minor: snapshot.tax_amount_minor,
        tax_rule: snapshot.tax_rule.try_into()?,
        promotion: snapshot.promotion.map(TryInto::try_into).transpose()?,
        tax_inclusive: snapshot.tax_inclusive,
        shipping: snapshot
            .shipping
            .map(ShippingSelection::try_from)
            .transpose()?,
        shipping_amount_minor: snapshot.shipping_amount_minor,
        total_amount_minor: snapshot.total_amount_minor,
        expires_at: parse_time(&snapshot.expires_at)?,
        lines: snapshot
            .lines
            .into_iter()
            .map(CheckoutLineItem::from)
            .collect(),
        created_at: parse_time(&snapshot.created_at)?,
    })
}

fn order_snapshot(detail: &OrderDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(OrderSnapshot {
        id: detail.id.as_uuid(),
        order_number: detail.order_number.as_str().into(),
        shopper_id: detail.shopper_id.as_uuid(),
        checkout_id: detail.checkout_id.as_uuid(),
        inventory_reservation_id: detail
            .inventory_reservation_id
            .map(InventoryReservationId::as_uuid),
        price_list_id: detail.price_list_id.as_uuid(),
        currency: detail.currency.as_str().into(),
        locale: detail.locale.as_str().into(),
        status: detail.status.as_str().into(),
        fulfillment_status: detail.fulfillment_status.as_str().into(),
        delivery_status: detail.delivery_status.as_str().into(),
        identity: CheckoutIdentitySnapshot::from(&detail.identity),
        subtotal_amount_minor: detail.subtotal_amount_minor,
        discount_amount_minor: detail.discount_amount_minor,
        tax_amount_minor: detail.tax_amount_minor,
        tax_rule: TaxRuleSnapshotData::from(&detail.tax_rule),
        promotion: detail
            .promotion
            .as_ref()
            .map(PromotionSnapshotData::try_from)
            .transpose()?,
        tax_inclusive: detail.tax_inclusive,
        shipping: detail
            .shipping
            .as_ref()
            .map(ShippingSelectionSnapshot::from),
        shipping_amount_minor: detail.shipping_amount_minor,
        total_amount_minor: detail.total_amount_minor,
        lines: detail.lines.iter().map(OrderLineSnapshot::from).collect(),
        transitions: detail
            .transitions
            .iter()
            .map(|item| {
                Ok(OrderTransitionSnapshot {
                    id: item.id,
                    from_status: item.from_status.map(|status| status.as_str().into()),
                    to_status: item.to_status.as_str().into(),
                    kind: item.kind.clone(),
                    actor_user_id: item.actor_user_id,
                    occurred_at: format_time(item.occurred_at)?,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_order(value: Value) -> Result<OrderDetail, ApplicationError> {
    let snapshot: OrderSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(OrderDetail {
        id: OrderId::from_uuid(snapshot.id),
        order_number: OrderNumber::parse(snapshot.order_number)?,
        shopper_id: ShopperId::from_uuid(snapshot.shopper_id),
        checkout_id: CheckoutId::from_uuid(snapshot.checkout_id),
        inventory_reservation_id: snapshot
            .inventory_reservation_id
            .map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(snapshot.price_list_id),
        currency: parse_currency(&snapshot.currency)?,
        locale: parse_locale(&snapshot.locale)?,
        status: OrderStatus::parse(&snapshot.status).ok_or_else(corrupt_sales_state)?,
        fulfillment_status: OrderFulfillmentStatus::parse(&snapshot.fulfillment_status)
            .ok_or_else(corrupt_sales_state)?,
        delivery_status: OrderDeliveryStatus::parse(&snapshot.delivery_status)
            .ok_or_else(corrupt_sales_state)?,
        identity: snapshot.identity.try_into()?,
        subtotal_amount_minor: snapshot.subtotal_amount_minor,
        discount_amount_minor: snapshot.discount_amount_minor,
        tax_amount_minor: snapshot.tax_amount_minor,
        tax_rule: snapshot.tax_rule.try_into()?,
        promotion: snapshot.promotion.map(TryInto::try_into).transpose()?,
        tax_inclusive: snapshot.tax_inclusive,
        shipping: snapshot
            .shipping
            .map(ShippingSelection::try_from)
            .transpose()?,
        shipping_amount_minor: snapshot.shipping_amount_minor,
        total_amount_minor: snapshot.total_amount_minor,
        lines: snapshot
            .lines
            .into_iter()
            .map(OrderLineItem::from)
            .collect(),
        transitions: snapshot
            .transitions
            .into_iter()
            .map(|item| {
                Ok(OrderTransitionItem {
                    id: item.id,
                    from_status: match item.from_status.as_deref() {
                        Some(status) => {
                            Some(OrderStatus::parse(status).ok_or_else(corrupt_sales_state)?)
                        }
                        None => None,
                    },
                    to_status: OrderStatus::parse(&item.to_status)
                        .ok_or_else(corrupt_sales_state)?,
                    kind: item.kind,
                    actor_user_id: item.actor_user_id,
                    occurred_at: parse_time(&item.occurred_at)?,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        created_at: parse_time(&snapshot.created_at)?,
        updated_at: parse_time(&snapshot.updated_at)?,
    })
}

impl From<&CheckoutIdentity> for CheckoutIdentitySnapshot {
    fn from(value: &CheckoutIdentity) -> Self {
        Self {
            contact: CheckoutContactSnapshot {
                email: value.contact().email().into(),
                phone: value.contact().phone().map(str::to_owned),
            },
            billing_address: PostalAddressSnapshot::from(value.billing_address()),
            shipping_address: value.shipping_address().map(PostalAddressSnapshot::from),
        }
    }
}

impl From<&ShippingSelection> for ShippingSelectionSnapshot {
    fn from(value: &ShippingSelection) -> Self {
        Self {
            service_id: value.service_id().as_uuid(),
            code: value.code().into(),
            name: value.name().into(),
            amount_minor: value.amount().amount_minor(),
            currency: value.amount().currency().as_str().into(),
            estimated_min_days: value.estimated_min_days(),
            estimated_max_days: value.estimated_max_days(),
        }
    }
}

impl TryFrom<ShippingSelectionSnapshot> for ShippingSelection {
    type Error = ApplicationError;

    fn try_from(value: ShippingSelectionSnapshot) -> Result<Self, Self::Error> {
        ShippingSelection::rehydrate(
            ShippingServiceId::from_uuid(value.service_id),
            value.code,
            value.name,
            Money::new(value.amount_minor, parse_currency(&value.currency)?),
            value.estimated_min_days,
            value.estimated_max_days,
        )
        .map_err(ApplicationError::from)
    }
}

impl From<&TaxRuleSnapshot> for TaxRuleSnapshotData {
    fn from(value: &TaxRuleSnapshot) -> Self {
        Self {
            rule_id: value.rule_id().as_uuid(),
            code: value.code().into(),
            name: value.name().into(),
            country_code: value.country_code().into(),
            rate_basis_points: value.rate_basis_points(),
        }
    }
}

impl TryFrom<TaxRuleSnapshotData> for TaxRuleSnapshot {
    type Error = ApplicationError;

    fn try_from(value: TaxRuleSnapshotData) -> Result<Self, Self::Error> {
        TaxRuleSnapshot::rehydrate(
            TaxRuleId::from_uuid(value.rule_id),
            value.code,
            value.name,
            value.country_code,
            value.rate_basis_points,
        )
        .map_err(ApplicationError::from)
    }
}

impl TryFrom<&PromotionSnapshot> for PromotionSnapshotData {
    type Error = ApplicationError;

    fn try_from(value: &PromotionSnapshot) -> Result<Self, Self::Error> {
        Ok(Self {
            promotion_id: value.promotion_id().as_uuid(),
            handle: value.handle().into(),
            name: value.name().into(),
            trigger: value.trigger().as_str().into(),
            redemption_code: value.redemption_code().map(Into::into),
            value_kind: value.value().kind().into(),
            rate_basis_points: value.value().rate_basis_points(),
            amount_minor: value.value().amount_minor(),
            maximum_amount_minor: value.value().maximum_amount_minor(),
            currency: value.currency().as_str().into(),
            minimum_subtotal_amount_minor: value.minimum_subtotal_amount_minor(),
            priority: value.priority(),
            starts_at: value.starts_at().map(format_time).transpose()?,
            ends_at: value.ends_at().map(format_time).transpose()?,
        })
    }
}

impl TryFrom<PromotionSnapshotData> for PromotionSnapshot {
    type Error = ApplicationError;

    fn try_from(value: PromotionSnapshotData) -> Result<Self, Self::Error> {
        let promotion_value = match value.value_kind.as_str() {
            "percentage" => PromotionValue::Percentage {
                rate_basis_points: value.rate_basis_points.ok_or_else(corrupt_sales_state)?,
                maximum_amount_minor: value.maximum_amount_minor,
            },
            "fixed_amount" => PromotionValue::FixedAmount {
                amount_minor: value.amount_minor.ok_or_else(corrupt_sales_state)?,
            },
            _ => return Err(corrupt_sales_state()),
        };
        PromotionSnapshot::rehydrate(
            PromotionId::from_uuid(value.promotion_id),
            value.handle,
            value.name,
            PromotionTrigger::parse(&value.trigger).ok_or_else(corrupt_sales_state)?,
            value.redemption_code,
            promotion_value,
            parse_currency(&value.currency)?,
            value.minimum_subtotal_amount_minor,
            value.priority,
            value.starts_at.as_deref().map(parse_time).transpose()?,
            value.ends_at.as_deref().map(parse_time).transpose()?,
        )
        .map_err(ApplicationError::from)
    }
}

impl TryFrom<CheckoutIdentitySnapshot> for CheckoutIdentity {
    type Error = ApplicationError;

    fn try_from(value: CheckoutIdentitySnapshot) -> Result<Self, Self::Error> {
        Ok(Self::new(
            CheckoutContact::new(value.contact.email, value.contact.phone)?,
            value.billing_address.try_into()?,
            value.shipping_address.map(TryInto::try_into).transpose()?,
        ))
    }
}

impl From<&PostalAddress> for PostalAddressSnapshot {
    fn from(value: &PostalAddress) -> Self {
        Self {
            full_name: value.full_name().into(),
            company: value.company().map(str::to_owned),
            address_line1: value.address_line1().into(),
            address_line2: value.address_line2().map(str::to_owned),
            locality: value.locality().into(),
            administrative_area: value.administrative_area().map(str::to_owned),
            postal_code: value.postal_code().map(str::to_owned),
            country_code: value.country_code().into(),
        }
    }
}

impl TryFrom<PostalAddressSnapshot> for PostalAddress {
    type Error = ApplicationError;

    fn try_from(value: PostalAddressSnapshot) -> Result<Self, Self::Error> {
        PostalAddress::new(
            value.full_name,
            value.company,
            value.address_line1,
            value.address_line2,
            value.locality,
            value.administrative_area,
            value.postal_code,
            value.country_code,
        )
        .map_err(ApplicationError::from)
    }
}

impl From<&OrderLineItem> for OrderLineSnapshot {
    fn from(value: &OrderLineItem) -> Self {
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
            discount_amount_minor: value.discount_amount_minor,
            tax_amount_minor: value.tax_amount_minor,
            total_amount_minor: value.total_amount_minor,
            tax_inclusive: value.tax_inclusive,
        }
    }
}

impl From<OrderLineSnapshot> for OrderLineItem {
    fn from(value: OrderLineSnapshot) -> Self {
        Self {
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
            discount_amount_minor: value.discount_amount_minor,
            tax_amount_minor: value.tax_amount_minor,
            total_amount_minor: value.total_amount_minor,
            tax_inclusive: value.tax_inclusive,
        }
    }
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
            tax_inclusive: value.tax_inclusive,
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
            tax_inclusive: value.tax_inclusive,
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

impl From<&CheckoutLineItem> for CheckoutLineSnapshot {
    fn from(value: &CheckoutLineItem) -> Self {
        Self {
            product_id: value.product_id.as_uuid(),
            product_variant_id: value.product_variant_id.as_uuid(),
            product_title: value.product_title.clone(),
            variant_title: value.variant_title.clone(),
            sku: value.sku.clone(),
            requires_shipping: value.requires_shipping,
            quantity: value.quantity,
            unit_price_amount_minor: value.unit_price_amount_minor,
            subtotal_amount_minor: value.subtotal_amount_minor,
            discount_amount_minor: value.discount_amount_minor,
            tax_amount_minor: value.tax_amount_minor,
            total_amount_minor: value.total_amount_minor,
            tax_inclusive: value.tax_inclusive,
        }
    }
}

impl From<CheckoutLineSnapshot> for CheckoutLineItem {
    fn from(value: CheckoutLineSnapshot) -> Self {
        Self {
            product_id: ProductId::from_uuid(value.product_id),
            product_variant_id: ProductVariantId::from_uuid(value.product_variant_id),
            product_title: value.product_title,
            variant_title: value.variant_title,
            sku: value.sku,
            requires_shipping: value.requires_shipping,
            quantity: value.quantity,
            unit_price_amount_minor: value.unit_price_amount_minor,
            subtotal_amount_minor: value.subtotal_amount_minor,
            discount_amount_minor: value.discount_amount_minor,
            tax_amount_minor: value.tax_amount_minor,
            total_amount_minor: value.total_amount_minor,
            tax_inclusive: value.tax_inclusive,
        }
    }
}

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

async fn ensure_checkout_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.checkouts \
         WHERE store_id = $1 AND sales_channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(checkout_id.as_uuid())
    .bind(shopper_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if owned {
        Ok(())
    } else {
        Err(checkout_not_found(checkout_id))
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

fn parse_locale(value: &str) -> Result<Locale, ApplicationError> {
    Locale::parse(value).map_err(Into::into)
}

fn default_locale_snapshot() -> String {
    "en-US".into()
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

fn checkout_not_found(checkout_id: CheckoutId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "checkout",
        id: checkout_id.as_uuid().to_string(),
    }
}

fn invalid_shipping_selection() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "shipping_service_id",
            reason: "must reference an active service for the Cart currency and destination".into(),
        }],
    }
}

fn invalid_promotion_code() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "promotion_code",
            reason: "must reference an active and eligible code for the Cart".into(),
        }],
    }
}

fn tax_rule_unavailable(country_code: &str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "tax_rule",
            reason: format!("no active Tax Rule is configured for destination {country_code}"),
        }],
    }
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn checkout_not_pending() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_not_pending",
        message: "the Checkout is no longer pending",
    }
}

fn checkout_already_pending() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_already_pending",
        message: "the Cart already has a pending Checkout",
    }
}

fn checkout_expired() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_expired",
        message: "the Checkout has expired",
    }
}

fn checkout_expiry_lease_lost() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_expiry_lease_lost",
        message: "the Checkout expiry lease is no longer owned by this worker",
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

#[cfg(test)]
#[path = "storefront_sales/tests.rs"]
mod tests;
