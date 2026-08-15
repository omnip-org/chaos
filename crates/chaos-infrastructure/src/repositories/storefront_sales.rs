use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        CartDetail, CartLineItem, CheckoutDetail, CheckoutLineItem, IdempotencyRequest,
        MachineActor, OrderDetail, OrderLineItem, OrderTransitionItem, StorefrontSalesRepository,
    },
};
use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    inventory::{InventoryReservationId, StockBalance},
    merchant::SalesChannelId,
    pricing::{Money, PriceListId},
    sales::{
        Cart, CartId, CartLine, CartStatus, Checkout, CheckoutId, CommercialAdjustments, Order,
        OrderId, OrderStatus,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_CART_OPERATION: &str = "carts.create.v1";
const SET_CART_LINE_OPERATION: &str = "cart_lines.set.v1";
const REMOVE_CART_LINE_OPERATION: &str = "cart_lines.remove.v1";
const CREATE_CHECKOUT_OPERATION: &str = "checkouts.create.v1";
const CREATE_ORDER_OPERATION: &str = "orders.create.v1";

type CartHeaderRow = (
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
    bool,
);
type CheckoutHeaderRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    Uuid,
    String,
    String,
    i64,
    i64,
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
    Option<Uuid>,
    Uuid,
    String,
    String,
    i64,
    i64,
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
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(actor.merchant_account_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

#[async_trait]
impl StorefrontSalesRepository for PostgresStorefrontSalesRepository {
    async fn create_cart(
        &self,
        actor: &MachineActor,
        currency: Option<CurrencyCode>,
        request: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError> {
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin(actor).await?;
        if let Some(snapshot) =
            reserve(&mut transaction, actor, CREATE_CART_OPERATION, request).await?
        {
            return replay_cart(snapshot);
        }
        let (price_list_id, currency) =
            select_price_list(&mut transaction, actor, channel_id, currency)
                .await?
                .ok_or_else(price_context_unavailable)?;
        let cart = Cart::create(
            actor.merchant_account_id,
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(price_list_id),
            currency,
        );
        sqlx::query(
            "INSERT INTO sales.carts \
             (id, merchant_account_id, store_id, sales_channel_id, price_list_id, currency) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(cart.id().as_uuid())
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(price_list_id)
        .bind(currency.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = load_cart(&mut transaction, actor, cart.id())
            .await?
            .ok_or_else(|| cart_not_found(cart.id()))?;
        complete(
            &mut transaction,
            actor,
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
        actor: &MachineActor,
        cart_id: CartId,
    ) -> Result<Option<CartDetail>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let detail = load_cart(&mut transaction, actor, cart_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn set_cart_line(
        &self,
        actor: &MachineActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        quantity: u32,
        request: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if let Some(snapshot) =
            reserve(&mut transaction, actor, SET_CART_LINE_OPERATION, request).await?
        {
            return replay_cart(snapshot);
        }
        let header = lock_active_cart(&mut transaction, actor, cart_id).await?;
        let currency = parse_currency(&header.2)?;
        let row = resolve_variant(
            &mut transaction,
            actor,
            SalesChannelId::from_uuid(header.0),
            PriceListId::from_uuid(header.1),
            product_variant_id,
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
        let detail = load_cart(&mut transaction, actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))?;
        complete(
            &mut transaction,
            actor,
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
        actor: &MachineActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        request: &IdempotencyRequest,
    ) -> Result<CartDetail, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if let Some(snapshot) =
            reserve(&mut transaction, actor, REMOVE_CART_LINE_OPERATION, request).await?
        {
            return replay_cart(snapshot);
        }
        lock_active_cart(&mut transaction, actor, cart_id).await?;
        sqlx::query(
            "DELETE FROM sales.cart_lines WHERE merchant_account_id = $1 AND store_id = $2 \
             AND cart_id = $3 AND product_variant_id = $4",
        )
        .bind(actor.merchant_account_id.as_uuid())
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
            actor,
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
        actor: &MachineActor,
        cart_id: CartId,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<CheckoutDetail, ApplicationError> {
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin(actor).await?;
        if let Some(snapshot) =
            reserve(&mut transaction, actor, CREATE_CHECKOUT_OPERATION, request).await?
        {
            return replay_checkout(snapshot);
        }
        let header = lock_active_cart(&mut transaction, actor, cart_id).await?;
        if header.0 != channel_id.as_uuid() {
            return Err(cart_not_found(cart_id));
        }
        let currency = parse_currency(&header.2)?;
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
            "SELECT count(*) FROM sales.cart_lines \
             WHERE merchant_account_id = $1 AND store_id = $2 AND cart_id = $3",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if usize::try_from(existing_count).ok() != Some(lines.len()) {
            return Err(cart_line_unavailable());
        }
        let cart = Cart::rehydrate(
            cart_id,
            actor.merchant_account_id,
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(header.1),
            currency,
            CartStatus::Active,
            lines,
        )?;
        let reservation_id =
            reserve_inventory(&mut transaction, actor, channel_id, &cart, expires_at).await?;
        let checkout = Checkout::freeze(
            &cart,
            reservation_id,
            expires_at,
            cart.lines()
                .iter()
                .map(|_| CommercialAdjustments::zero(currency))
                .collect(),
        )?;
        insert_checkout(&mut transaction, actor, channel_id, &checkout).await?;
        let result = sqlx::query(
            "UPDATE sales.carts SET status = 'completed', version = version + 1, \
                    updated_at = CURRENT_TIMESTAMP \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 AND status = 'active'",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(cart_not_active());
        }
        let detail = load_checkout(&mut transaction, actor, checkout.id())
            .await?
            .ok_or_else(|| checkout_not_found(checkout.id()))?;
        complete(
            &mut transaction,
            actor,
            CREATE_CHECKOUT_OPERATION,
            request,
            201,
            checkout_snapshot(&detail)?,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn get_checkout(
        &self,
        actor: &MachineActor,
        checkout_id: CheckoutId,
    ) -> Result<Option<CheckoutDetail>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let detail = load_checkout(&mut transaction, actor, checkout_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn create_order(
        &self,
        actor: &MachineActor,
        checkout_id: CheckoutId,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<OrderDetail, ApplicationError> {
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin(actor).await?;
        if let Some(snapshot) =
            reserve(&mut transaction, actor, CREATE_ORDER_OPERATION, request).await?
        {
            return replay_order(snapshot);
        }
        let checkout = sqlx::query_as::<_, (String, OffsetDateTime)>(
            "SELECT status::text, expires_at FROM sales.checkouts \
             WHERE merchant_account_id = $1 AND store_id = $2 AND sales_channel_id = $3 \
               AND id = $4 FOR UPDATE",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(checkout_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| checkout_not_found(checkout_id))?;
        if checkout.0 != "pending" {
            return Err(checkout_not_pending());
        }
        if checkout.1 <= now {
            return Err(checkout_expired());
        }
        let order = Order::create(checkout_id);
        sqlx::query(
            "INSERT INTO sales.orders \
             (id, merchant_account_id, store_id, sales_channel_id, checkout_id, \
              inventory_reservation_id, price_list_id, currency, subtotal_amount_minor, \
              discount_amount_minor, tax_amount_minor, total_amount_minor, created_at, updated_at) \
             SELECT $5, merchant_account_id, store_id, sales_channel_id, id, \
                    inventory_reservation_id, price_list_id, currency, subtotal_amount_minor, \
                    discount_amount_minor, tax_amount_minor, total_amount_minor, $6, $6 \
             FROM sales.checkouts WHERE merchant_account_id = $1 AND store_id = $2 \
               AND sales_channel_id = $3 AND id = $4",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(checkout_id.as_uuid())
        .bind(order.id().as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO sales.order_lines \
             (merchant_account_id, store_id, order_id, position, product_id, \
              product_variant_id, product_title, variant_title, sku, requires_shipping, \
              track_inventory, quantity, unit_price_amount_minor, subtotal_amount_minor, \
              discount_amount_minor, tax_amount_minor, total_amount_minor, tax_inclusive, created_at) \
             SELECT merchant_account_id, store_id, $4, position, product_id, product_variant_id, \
                    product_title, variant_title, sku, requires_shipping, track_inventory, quantity, \
                    unit_price_amount_minor, subtotal_amount_minor, discount_amount_minor, \
                    tax_amount_minor, total_amount_minor, tax_inclusive, $5 \
             FROM sales.checkout_lines WHERE merchant_account_id = $1 AND store_id = $2 \
               AND checkout_id = $3 ORDER BY position",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(checkout_id.as_uuid())
        .bind(order.id().as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO sales.order_transitions \
             (id, merchant_account_id, store_id, order_id, from_status, to_status, kind, occurred_at) \
             VALUES ($1, $2, $3, $4, NULL, 'pending', 'created', $5)",
        )
        .bind(Uuid::now_v7())
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(order.id().as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "UPDATE sales.checkouts SET status = 'completed', closed_at = $5, updated_at = $5 \
             WHERE merchant_account_id = $1 AND store_id = $2 AND sales_channel_id = $3 \
               AND id = $4 AND status = 'pending'",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(checkout_id.as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = load_order(&mut transaction, actor, order.id())
            .await?
            .ok_or_else(|| order_not_found(order.id()))?;
        complete(
            &mut transaction,
            actor,
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
        actor: &MachineActor,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let detail = load_order(&mut transaction, actor, order_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
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
         FROM pricing.price_lists AS price_list \
         INNER JOIN merchant.stores AS store \
           ON store.merchant_account_id = price_list.merchant_account_id \
          AND store.id = price_list.store_id \
         INNER JOIN merchant.sales_channels AS channel \
           ON channel.merchant_account_id = store.merchant_account_id \
          AND channel.store_id = store.id AND channel.id = $3 \
         INNER JOIN merchant.store_currencies AS store_currency \
           ON store_currency.merchant_account_id = store.merchant_account_id \
          AND store_currency.store_id = store.id \
          AND store_currency.currency = price_list.currency \
         WHERE price_list.merchant_account_id = $1 AND price_list.store_id = $2 \
           AND store.status = 'active' AND channel.status = 'active' \
           AND price_list.status = 'active' AND store_currency.enabled \
           AND price_list.currency = COALESCE($4::char(3), store.default_currency) \
           AND (price_list.starts_at IS NULL OR price_list.starts_at <= CURRENT_TIMESTAMP) \
           AND (price_list.ends_at IS NULL OR price_list.ends_at > CURRENT_TIMESTAMP) \
         ORDER BY price_list.starts_at DESC NULLS LAST, price_list.id ASC LIMIT 1",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(currency.map(|value| value.as_str().to_owned()))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|(id, currency)| Ok((id, parse_currency(&currency)?)))
        .transpose()
}

async fn resolve_variant(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    channel_id: SalesChannelId,
    price_list_id: PriceListId,
    variant_id: ProductVariantId,
) -> Result<Option<(Uuid, String, String, Option<String>, bool, bool, i64, bool)>, ApplicationError>
{
    sqlx::query_as(
        "SELECT product.id, product.title, variant.title, variant.sku::text, \
                variant.requires_shipping, variant.track_inventory, price.amount_minor, \
                price_list.tax_inclusive \
         FROM catalog.product_variants AS variant \
         INNER JOIN catalog.products AS product \
           ON product.merchant_account_id = variant.merchant_account_id \
          AND product.store_id = variant.store_id AND product.id = variant.product_id \
         INNER JOIN catalog.product_publications AS publication \
           ON publication.merchant_account_id = product.merchant_account_id \
          AND publication.store_id = product.store_id AND publication.product_id = product.id \
          AND publication.sales_channel_id = $3 \
         INNER JOIN pricing.price_lists AS price_list \
           ON price_list.merchant_account_id = variant.merchant_account_id \
          AND price_list.store_id = variant.store_id AND price_list.id = $4 \
         INNER JOIN pricing.prices AS price \
           ON price.merchant_account_id = variant.merchant_account_id \
          AND price.store_id = variant.store_id AND price.price_list_id = price_list.id \
          AND price.product_variant_id = variant.id \
         WHERE variant.merchant_account_id = $1 AND variant.store_id = $2 AND variant.id = $5 \
           AND variant.status = 'active' AND product.status = 'active' \
           AND price_list.status = 'active' \
           AND (price_list.starts_at IS NULL OR price_list.starts_at <= CURRENT_TIMESTAMP) \
           AND (price_list.ends_at IS NULL OR price_list.ends_at > CURRENT_TIMESTAMP)",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(price_list_id.as_uuid())
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
        "INSERT INTO sales.cart_lines \
         (merchant_account_id, store_id, cart_id, product_id, product_variant_id, \
          product_title, variant_title, sku, requires_shipping, track_inventory, quantity, \
          unit_price_amount_minor, tax_inclusive) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (merchant_account_id, store_id, cart_id, product_variant_id) \
         DO UPDATE SET product_title = EXCLUDED.product_title, \
             variant_title = EXCLUDED.variant_title, sku = EXCLUDED.sku, \
             requires_shipping = EXCLUDED.requires_shipping, \
             track_inventory = EXCLUDED.track_inventory, quantity = EXCLUDED.quantity, \
             unit_price_amount_minor = EXCLUDED.unit_price_amount_minor, \
             tax_inclusive = EXCLUDED.tax_inclusive, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(actor.merchant_account_id.as_uuid())
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
        "UPDATE sales.carts SET version = version + 1, updated_at = CURRENT_TIMESTAMP \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(actor.merchant_account_id.as_uuid())
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
) -> Result<(Uuid, Uuid, String), ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
        "SELECT sales_channel_id, price_list_id, currency::text, status::text \
         FROM sales.carts WHERE merchant_account_id = $1 AND store_id = $2 \
           AND sales_channel_id = $3 AND id = $4 FOR UPDATE",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| cart_not_found(cart_id))?;
    if row.3 != "active" {
        return Err(cart_not_active());
    }
    Ok((row.0, row.1, row.2))
}

async fn load_cart(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<Option<CartDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, CartHeaderRow>(
        "SELECT id, price_list_id, currency::text, status::text, version, created_at, updated_at \
         FROM sales.carts WHERE merchant_account_id = $1 AND store_id = $2 \
           AND sales_channel_id = $3 AND id = $4",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let currency = parse_currency(&row.2)?;
    let status = CartStatus::parse(&row.3).ok_or_else(corrupt_sales_state)?;
    let lines = load_cart_line_rows(transaction, actor, cart_id).await?;
    let items = lines
        .into_iter()
        .map(|line| cart_line_item(line, currency))
        .collect::<Result<Vec<_>, _>>()?;
    let subtotal = items
        .iter()
        .try_fold(Money::zero(currency), |total, line| {
            total.checked_add(&Money::new(line.subtotal_amount_minor, currency))
        })?;
    Ok(Some(CartDetail {
        id: CartId::from_uuid(row.0),
        price_list_id: PriceListId::from_uuid(row.1),
        currency,
        status,
        version: u64::try_from(row.4).map_err(unexpected_conversion)?,
        lines: items,
        subtotal_amount_minor: subtotal.amount_minor(),
        created_at: row.5,
        updated_at: row.6,
    }))
}

async fn load_cart_line_rows(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<Vec<CartLineRow>, ApplicationError> {
    sqlx::query_as(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor, \
                tax_inclusive FROM sales.cart_lines \
         WHERE merchant_account_id = $1 AND store_id = $2 AND cart_id = $3 \
         ORDER BY product_variant_id ASC",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(cart_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)
}

fn cart_line_item(
    row: CartLineRow,
    currency: CurrencyCode,
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
        "SELECT product.id, variant.id, product.title, variant.title, variant.sku::text, \
                variant.requires_shipping, variant.track_inventory, cart_line.quantity, \
                price.amount_minor, price_list.tax_inclusive \
         FROM sales.cart_lines AS cart_line \
         INNER JOIN catalog.product_variants AS variant \
           ON variant.merchant_account_id = cart_line.merchant_account_id \
          AND variant.store_id = cart_line.store_id \
          AND variant.id = cart_line.product_variant_id AND variant.status = 'active' \
         INNER JOIN catalog.products AS product \
           ON product.merchant_account_id = variant.merchant_account_id \
          AND product.store_id = variant.store_id AND product.id = variant.product_id \
          AND product.status = 'active' \
         INNER JOIN catalog.product_publications AS publication \
           ON publication.merchant_account_id = product.merchant_account_id \
          AND publication.store_id = product.store_id AND publication.product_id = product.id \
          AND publication.sales_channel_id = $4 \
         INNER JOIN pricing.price_lists AS price_list \
           ON price_list.merchant_account_id = cart_line.merchant_account_id \
          AND price_list.store_id = cart_line.store_id AND price_list.id = $5 \
         INNER JOIN pricing.prices AS price \
           ON price.merchant_account_id = variant.merchant_account_id \
          AND price.store_id = variant.store_id AND price.price_list_id = price_list.id \
          AND price.product_variant_id = variant.id \
         WHERE cart_line.merchant_account_id = $1 AND cart_line.store_id = $2 \
           AND cart_line.cart_id = $3 ORDER BY variant.id ASC",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(cart_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(price_list_id.as_uuid())
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
        "SELECT EXISTS (SELECT 1 FROM pricing.price_lists \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 \
           AND currency = $4 AND status = 'active' \
           AND (starts_at IS NULL OR starts_at <= $5) \
           AND (ends_at IS NULL OR ends_at > $5))",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(price_list_id.as_uuid())
    .bind(currency.as_str())
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
        "INSERT INTO inventory.inventory_reservations \
         (id, merchant_account_id, store_id, sales_channel_id, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(reservation_id.as_uuid())
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(expires_at)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    for line in cart.lines().iter().filter(|line| line.track_inventory()) {
        let stocks = sqlx::query_as::<_, (Uuid, i64, i64)>(
            "SELECT stock.id, stock.on_hand_quantity, stock.reserved_quantity \
             FROM inventory.stock_items AS stock \
             INNER JOIN inventory.inventory_locations AS location \
               ON location.merchant_account_id = stock.merchant_account_id \
              AND location.store_id = stock.store_id AND location.id = stock.inventory_location_id \
             WHERE stock.merchant_account_id = $1 AND stock.store_id = $2 \
               AND stock.product_variant_id = $3 AND location.status = 'active' \
               AND stock.on_hand_quantity > stock.reserved_quantity \
             ORDER BY stock.id ASC FOR UPDATE OF stock",
        )
        .bind(actor.merchant_account_id.as_uuid())
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
                "UPDATE inventory.stock_items SET reserved_quantity = $2, \
                        updated_at = CURRENT_TIMESTAMP WHERE id = $1",
            )
            .bind(stock_item_id)
            .bind(balance.reserved())
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO inventory.inventory_reservation_lines \
                 (merchant_account_id, store_id, reservation_id, stock_item_id, \
                  product_variant_id, quantity) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(actor.merchant_account_id.as_uuid())
            .bind(actor.store_id.as_uuid())
            .bind(reservation_id.as_uuid())
            .bind(stock_item_id)
            .bind(line.product_variant_id().as_uuid())
            .bind(allocated)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO inventory.stock_ledger_entries \
                 (id, merchant_account_id, store_id, stock_item_id, reservation_id, kind, \
                  on_hand_delta_quantity, reserved_delta_quantity, resulting_on_hand_quantity, \
                  resulting_reserved_quantity) \
                 VALUES ($1, $2, $3, $4, $5, 'reservation_created', 0, $6, $7, $8)",
            )
            .bind(Uuid::now_v7())
            .bind(actor.merchant_account_id.as_uuid())
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
    channel_id: SalesChannelId,
    checkout: &Checkout,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO sales.checkouts \
         (id, merchant_account_id, store_id, cart_id, sales_channel_id, price_list_id, \
          inventory_reservation_id, currency, subtotal_amount_minor, discount_amount_minor, \
          tax_amount_minor, total_amount_minor, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(checkout.id().as_uuid())
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(checkout.cart_id().as_uuid())
    .bind(channel_id.as_uuid())
    .bind(checkout.price_list_id().as_uuid())
    .bind(
        checkout
            .reservation_id()
            .map(InventoryReservationId::as_uuid),
    )
    .bind(checkout.currency().as_str())
    .bind(checkout.subtotal().amount_minor())
    .bind(checkout.discount().amount_minor())
    .bind(checkout.tax().amount_minor())
    .bind(checkout.total().amount_minor())
    .bind(checkout.expires_at())
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    for (position, line) in checkout.lines().iter().enumerate() {
        let cart_line = line.cart_line();
        sqlx::query(
            "INSERT INTO sales.checkout_lines \
             (merchant_account_id, store_id, checkout_id, position, product_id, \
              product_variant_id, product_title, variant_title, sku, requires_shipping, \
              track_inventory, quantity, unit_price_amount_minor, subtotal_amount_minor, \
              discount_amount_minor, tax_amount_minor, total_amount_minor, tax_inclusive) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
                     $14, $15, $16, $17, $18)",
        )
        .bind(actor.merchant_account_id.as_uuid())
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

async fn load_checkout(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
) -> Result<Option<CheckoutDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, CheckoutHeaderRow>(
        "SELECT id, cart_id, inventory_reservation_id, price_list_id, currency::text, \
                status::text, subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                total_amount_minor, expires_at, created_at FROM sales.checkouts \
         WHERE merchant_account_id = $1 AND store_id = $2 AND sales_channel_id = $3 AND id = $4",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(checkout_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let lines = sqlx::query_as::<_, CheckoutLineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, quantity, unit_price_amount_minor, subtotal_amount_minor, \
                discount_amount_minor, tax_amount_minor, total_amount_minor, tax_inclusive \
         FROM sales.checkout_lines WHERE merchant_account_id = $1 AND store_id = $2 \
           AND checkout_id = $3 ORDER BY position ASC",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(checkout_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(Some(CheckoutDetail {
        id: CheckoutId::from_uuid(row.0),
        cart_id: CartId::from_uuid(row.1),
        inventory_reservation_id: row.2.map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(row.3),
        currency: parse_currency(&row.4)?,
        status: row.5,
        subtotal_amount_minor: row.6,
        discount_amount_minor: row.7,
        tax_amount_minor: row.8,
        total_amount_minor: row.9,
        expires_at: row.10,
        lines: lines
            .into_iter()
            .map(checkout_line_item)
            .collect::<Result<Vec<_>, _>>()?,
        created_at: row.11,
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

async fn load_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, OrderHeaderRow>(
        "SELECT id, checkout_id, inventory_reservation_id, price_list_id, currency::text, \
                status::text, subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                total_amount_minor, created_at, updated_at FROM sales.orders \
         WHERE merchant_account_id = $1 AND store_id = $2 AND sales_channel_id = $3 AND id = $4",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let lines = sqlx::query_as::<_, OrderLineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor, \
                subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                total_amount_minor, tax_inclusive FROM sales.order_lines \
         WHERE merchant_account_id = $1 AND store_id = $2 AND order_id = $3 ORDER BY position",
    )
    .bind(actor.merchant_account_id.as_uuid())
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
         FROM sales.order_transitions WHERE merchant_account_id = $1 AND store_id = $2 \
           AND order_id = $3 ORDER BY occurred_at, id",
    )
    .bind(actor.merchant_account_id.as_uuid())
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(Some(OrderDetail {
        id: OrderId::from_uuid(row.0),
        checkout_id: CheckoutId::from_uuid(row.1),
        inventory_reservation_id: row.2.map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(row.3),
        currency: parse_currency(&row.4)?,
        status: OrderStatus::parse(&row.5).ok_or_else(corrupt_sales_state)?,
        subtotal_amount_minor: row.6,
        discount_amount_minor: row.7,
        tax_amount_minor: row.8,
        total_amount_minor: row.9,
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
        created_at: row.10,
        updated_at: row.11,
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
    tax_inclusive: bool,
}

#[derive(Serialize, Deserialize)]
struct CheckoutSnapshot {
    id: Uuid,
    cart_id: Uuid,
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    status: String,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
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
    checkout_id: Uuid,
    inventory_reservation_id: Option<Uuid>,
    price_list_id: Uuid,
    currency: String,
    status: String,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
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

fn cart_snapshot(detail: &CartDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(CartSnapshot {
        id: detail.id.as_uuid(),
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
        price_list_id: PriceListId::from_uuid(snapshot.price_list_id),
        currency: parse_currency(&snapshot.currency)?,
        status: CartStatus::parse(&snapshot.status).ok_or_else(corrupt_sales_state)?,
        version: snapshot.version,
        lines: snapshot.lines.into_iter().map(CartLineItem::from).collect(),
        subtotal_amount_minor: snapshot.subtotal_amount_minor,
        created_at: parse_time(&snapshot.created_at)?,
        updated_at: parse_time(&snapshot.updated_at)?,
    })
}

fn checkout_snapshot(detail: &CheckoutDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(CheckoutSnapshot {
        id: detail.id.as_uuid(),
        cart_id: detail.cart_id.as_uuid(),
        inventory_reservation_id: detail
            .inventory_reservation_id
            .map(InventoryReservationId::as_uuid),
        price_list_id: detail.price_list_id.as_uuid(),
        currency: detail.currency.as_str().into(),
        status: detail.status.clone(),
        subtotal_amount_minor: detail.subtotal_amount_minor,
        discount_amount_minor: detail.discount_amount_minor,
        tax_amount_minor: detail.tax_amount_minor,
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
        cart_id: CartId::from_uuid(snapshot.cart_id),
        inventory_reservation_id: snapshot
            .inventory_reservation_id
            .map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(snapshot.price_list_id),
        currency: parse_currency(&snapshot.currency)?,
        status: snapshot.status,
        subtotal_amount_minor: snapshot.subtotal_amount_minor,
        discount_amount_minor: snapshot.discount_amount_minor,
        tax_amount_minor: snapshot.tax_amount_minor,
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
        checkout_id: detail.checkout_id.as_uuid(),
        inventory_reservation_id: detail
            .inventory_reservation_id
            .map(InventoryReservationId::as_uuid),
        price_list_id: detail.price_list_id.as_uuid(),
        currency: detail.currency.as_str().into(),
        status: detail.status.as_str().into(),
        subtotal_amount_minor: detail.subtotal_amount_minor,
        discount_amount_minor: detail.discount_amount_minor,
        tax_amount_minor: detail.tax_amount_minor,
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
        checkout_id: CheckoutId::from_uuid(snapshot.checkout_id),
        inventory_reservation_id: snapshot
            .inventory_reservation_id
            .map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(snapshot.price_list_id),
        currency: parse_currency(&snapshot.currency)?,
        status: OrderStatus::parse(&snapshot.status).ok_or_else(corrupt_sales_state)?,
        subtotal_amount_minor: snapshot.subtotal_amount_minor,
        discount_amount_minor: snapshot.discount_amount_minor,
        tax_amount_minor: snapshot.tax_amount_minor,
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
        }
    }
}

impl From<CartLineSnapshot> for CartLineItem {
    fn from(value: CartLineSnapshot) -> Self {
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
            tax_inclusive: value.tax_inclusive,
        }
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

async fn reserve(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Value>, ApplicationError> {
    idempotency::reserve(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id.as_uuid()),
        operation,
        request,
    )
    .await
}

async fn complete(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    status: i16,
    snapshot: Value,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id.as_uuid()),
        operation,
        request,
        status,
        snapshot,
    )
    .await
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

fn checkout_not_found(checkout_id: CheckoutId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "checkout",
        id: checkout_id.as_uuid().to_string(),
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

fn checkout_expired() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_expired",
        message: "the Checkout has expired",
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
mod tests {
    use std::sync::Arc;

    use chaos_application::{
        ports::IdempotencyRequest,
        sales::{CreateCartInput, CreateCheckoutInput, SetCartLineInput, StorefrontSales},
    };
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        identity::UserId,
        inventory::InventoryLocationId,
        merchant::{
            ApiKeyClass, ApiKeyId, ApiKeyMode, ApiKeyScope, MerchantAccountId, SalesChannelId,
            StoreId,
        },
    };
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    fn request(key: impl Into<String>, fingerprint: u8) -> IdempotencyRequest {
        IdempotencyRequest {
            key: key.into(),
            request_fingerprint: [fingerprint; 32],
        }
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn cart_checkout_is_idempotent_atomic_isolated_and_inventory_safe() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(3)
            .connect(&database_url)
            .await
            .unwrap();
        let runtime_pool = PgPoolOptions::new()
            .max_connections(8)
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("SET ROLE chaos_runtime")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .unwrap();
        let suffix = Uuid::now_v7().simple().to_string();
        let user_id = UserId::new();
        let account_id = MerchantAccountId::new();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let other_channel_id = SalesChannelId::new();
        let product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let price_list_id = PriceListId::new();
        let location_id = InventoryLocationId::new();
        let stock_item_id = Uuid::now_v7();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id.as_uuid())
            .bind(format!("sales-repository-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Sales Repository Test')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("sales-repository-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, code) in [
            (store_id, "sales-store"),
            (other_store_id, "other-sales-store"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.stores \
                 (id, merchant_account_id, code, name, status) \
                 VALUES ($1, $2, $3, 'Sales Store', 'active')",
            )
            .bind(id.as_uuid())
            .bind(account_id.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO merchant.store_currencies \
                 (merchant_account_id, store_id, currency) VALUES ($1, $2, 'USD')",
            )
            .bind(account_id.as_uuid())
            .bind(id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (id, store, code) in [
            (channel_id, store_id, "web"),
            (other_channel_id, other_store_id, "other-web"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.sales_channels \
                 (id, merchant_account_id, store_id, code, name, kind, is_default) \
                 VALUES ($1, $2, $3, $4, 'Web', 'web', true)",
            )
            .bind(id.as_uuid())
            .bind(account_id.as_uuid())
            .bind(store.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO catalog.products \
             (id, merchant_account_id, store_id, handle, title, status) \
             VALUES ($1, $2, $3, 'checkout-product', 'Checkout Product', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, merchant_account_id, store_id, product_id, title, sku, status, track_inventory) \
             VALUES ($1, $2, $3, $4, 'Default', 'CHECKOUT-SKU', 'active', true)",
        )
        .bind(variant_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_publications \
             (merchant_account_id, store_id, product_id, sales_channel_id) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(channel_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.price_lists \
             (id, merchant_account_id, store_id, code, name, currency, status) \
             VALUES ($1, $2, $3, 'default-usd', 'Default USD', 'USD', 'active')",
        )
        .bind(price_list_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.prices \
             (id, merchant_account_id, store_id, price_list_id, product_variant_id, amount_minor) \
             VALUES ($1, $2, $3, $4, $5, 1250)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(price_list_id.as_uuid())
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO inventory.inventory_locations \
             (id, merchant_account_id, store_id, code, name) \
             VALUES ($1, $2, $3, 'primary', 'Primary')",
        )
        .bind(location_id.as_uuid())
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO inventory.stock_items \
             (id, merchant_account_id, store_id, inventory_location_id, product_variant_id, \
              on_hand_quantity) VALUES ($1, $2, $3, $4, $5, 5)",
        )
        .bind(stock_item_id)
        .bind(account_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(location_id.as_uuid())
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let actor = MachineActor {
            api_key_id: ApiKeyId::new(),
            merchant_account_id: account_id,
            store_id,
            sales_channel_id: Some(channel_id),
            class: ApiKeyClass::Publishable,
            mode: ApiKeyMode::Live,
            scopes: vec![ApiKeyScope::CartsWrite, ApiKeyScope::CheckoutWrite],
        };
        let service = Arc::new(StorefrontSales::new(Arc::new(
            PostgresStorefrontSalesRepository::new(runtime_pool.clone()),
        )));
        let cart = service
            .create_cart(CreateCartInput {
                actor: actor.clone(),
                currency: None,
                idempotency: request(format!("create-cart-{suffix}"), 1),
            })
            .await
            .unwrap();
        assert!(cart.lines.is_empty());
        let updated = service
            .set_cart_line(SetCartLineInput {
                actor: actor.clone(),
                cart_id: cart.id,
                product_variant_id: variant_id,
                quantity: 3,
                idempotency: request(format!("set-line-{suffix}"), 2),
            })
            .await
            .unwrap();
        assert_eq!(updated.subtotal_amount_minor, 3_750);
        assert_eq!(updated.version, 1);
        let replayed_creation = service
            .create_cart(CreateCartInput {
                actor: actor.clone(),
                currency: None,
                idempotency: request(format!("create-cart-{suffix}"), 1),
            })
            .await
            .unwrap();
        assert!(replayed_creation.lines.is_empty());
        assert_eq!(replayed_creation.version, 0);

        let now = OffsetDateTime::now_utc();
        let first_service = service.clone();
        let first_actor = actor.clone();
        let first_key = format!("checkout-{suffix}");
        let first = tokio::spawn(async move {
            first_service
                .create_checkout(CreateCheckoutInput {
                    actor: first_actor,
                    cart_id: cart.id,
                    now,
                    idempotency: request(first_key, 3),
                })
                .await
        });
        let second_service = service.clone();
        let second_actor = actor.clone();
        let second_key = format!("checkout-{suffix}");
        let second = tokio::spawn(async move {
            second_service
                .create_checkout(CreateCheckoutInput {
                    actor: second_actor,
                    cart_id: cart.id,
                    now,
                    idempotency: request(second_key, 3),
                })
                .await
        });
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.total_amount_minor, 3_750);
        assert_eq!(first.lines[0].product_title, "Checkout Product");
        assert!(first.inventory_reservation_id.is_some());

        let stock: (i64, i64) = sqlx::query_as(
            "SELECT on_hand_quantity, reserved_quantity \
             FROM inventory.stock_items WHERE id = $1",
        )
        .bind(stock_item_id)
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(stock, (5, 3));
        let ledger_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM inventory.stock_ledger_entries \
             WHERE reservation_id = $1",
        )
        .bind(first.inventory_reservation_id.unwrap().as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(ledger_count, 1);

        let other_actor = MachineActor {
            store_id: other_store_id,
            sales_channel_id: Some(other_channel_id),
            ..actor.clone()
        };
        assert!(service.get_cart(&other_actor, cart.id).await.is_err());
        assert!(
            service
                .create_checkout(CreateCheckoutInput {
                    actor,
                    cart_id: cart.id,
                    now,
                    idempotency: request(format!("second-checkout-{suffix}"), 4),
                })
                .await
                .is_err()
        );

        let mut runtime_connection = runtime_pool.acquire().await.unwrap();
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, false)")
            .bind(account_id.as_uuid().to_string())
            .execute(&mut *runtime_connection)
            .await
            .unwrap();
        assert!(
            sqlx::query(
                "UPDATE sales.checkout_lines SET product_title = 'Tampered' \
                 WHERE checkout_id = $1",
            )
            .bind(first.id.as_uuid())
            .execute(&mut *runtime_connection)
            .await
            .is_err()
        );
    }
}
