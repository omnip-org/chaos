use chaos_domain::{
    catalog::{ProductId, ProductVariantId},
    pricing::{Money, PriceListId},
    sales::{Cart, CartId, CartLine, CartStatus, OrderId, OrderNumber, ShopperId},
    store::SalesChannelId,
};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApplicationError,
    contracts::{CartDetail, CheckoutActor, MachineActor, OrderDetail, ShopperActor},
    error::database_error,
    sales::CheckoutRequest,
};

use super::{
    cart::{
        bump_cart, insert_or_replace_line, load_cart, load_cart_media, lock_active_cart, lock_cart,
        refresh_cart_lines, require_price_list_active, resolve_variant, select_price_list,
    },
    repository::*,
};

#[derive(sqlx::FromRow)]
struct ExistingCheckoutRow {
    order_id: Uuid,
    order_status: String,
    idempotency_key: Option<Uuid>,
    payment_status: String,
    request_fingerprint: Option<Vec<u8>>,
}

async fn reserve_inventory_for_cart(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart: &Cart,
) -> Result<(), ApplicationError> {
    for line in cart.lines().iter().filter(|line| line.track_inventory()) {
        let reserved: Option<Uuid> = sqlx::query_scalar(
            "UPDATE chaos_commerce.product_variants \
             SET reserved_quantity = reserved_quantity + $3, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2 AND track_inventory \
               AND on_hand_quantity - reserved_quantity >= $3 \
             RETURNING id",
        )
        .bind(actor.store_id.as_uuid())
        .bind(line.product_variant_id().as_uuid())
        .bind(i64::from(line.quantity()))
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
        if reserved.is_none() {
            return Err(insufficient_inventory(line.product_variant_id()));
        }
    }
    Ok(())
}

impl PostgresStorefrontSalesRepository {
    pub(crate) async fn create_shopper(
        &self,
        actor: &MachineActor,
        attribution: Option<Value>,
    ) -> Result<ShopperId, ApplicationError> {
        require_channel(actor)?;
        let shopper_id = ShopperId::new();
        let mut transaction = self.begin(actor).await?;
        sqlx::query(
            "INSERT INTO chaos_commerce.shoppers (id, store_id, attribution) VALUES ($1, $2, $3)",
        )
        .bind(shopper_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(attribution)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(shopper_id)
    }

    /// Replaces `last_seen` while preserving the original `first_seen` snapshot.
    pub(crate) async fn refresh_shopper_last_seen(
        &self,
        shopper: &ShopperActor,
        snapshot: Value,
    ) -> Result<(), ApplicationError> {
        require_channel(&shopper.machine)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        sqlx::query(
            "UPDATE chaos_commerce.shoppers \
             SET attribution = jsonb_set(COALESCE(attribution, '{}'::jsonb), '{last_seen}', $3), \
                 updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(shopper.machine.store_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(snapshot)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn create_cart(
        &self,
        shopper: &ShopperActor,
    ) -> Result<CartDetail, ApplicationError> {
        let shopper_id = shopper.shopper_id;
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;

        // Repeated creates return the active Cart guarded by the partial unique index.
        if let Some(cart_id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM chaos_commerce.carts \
             WHERE store_id = $1 AND channel_id = $2 AND shopper_id = $3 \
               AND status = 'active' \
             ORDER BY updated_at DESC, id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        {
            let detail = load_cart(&mut transaction, actor, CartId::from_uuid(cart_id))
                .await?
                .ok_or_else(|| cart_not_found(CartId::from_uuid(cart_id)))?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(detail);
        }

        let (price_list_id, currency) = select_price_list(&mut transaction, actor, channel_id)
            .await?
            .ok_or_else(price_context_unavailable)?;
        let cart = Cart::create(
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(price_list_id),
            currency,
        );
        sqlx::query(
            "INSERT INTO chaos_commerce.carts \
             (id, store_id, shopper_id, channel_id, price_list_id) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (store_id, channel_id, shopper_id) \
                 WHERE status = 'active' DO NOTHING",
        )
        .bind(cart.id().as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(shopper_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(price_list_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let canonical_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM chaos_commerce.carts \
             WHERE store_id = $1 AND channel_id = $2 AND shopper_id = $3 \
               AND status = 'active' \
             ORDER BY updated_at DESC, id DESC LIMIT 1",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| cart_not_found(cart.id()))?;
        let detail = load_cart(&mut transaction, actor, CartId::from_uuid(canonical_id))
            .await?
            .ok_or_else(|| cart_not_found(CartId::from_uuid(canonical_id)))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn get_active_cart(
        &self,
        shopper: &ShopperActor,
    ) -> Result<Option<CartDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let cart_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM chaos_commerce.carts \
             WHERE store_id = $1 AND channel_id = $2 AND shopper_id = $3 \
               AND status = 'active' \
             ORDER BY updated_at DESC, id DESC LIMIT 1",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = match cart_id {
            Some(cart_id) => load_cart(&mut transaction, actor, CartId::from_uuid(cart_id)).await?,
            None => None,
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn get_cart(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
    ) -> Result<CartDetail, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        let detail = load_cart(&mut transaction, actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn set_cart_line(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
        quantity: u32,
    ) -> Result<CartDetail, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        let header = lock_active_cart(&mut transaction, actor, cart_id).await?;
        let currency = parse_currency(&header.currency)?;
        let row = resolve_variant(
            &mut transaction,
            actor,
            SalesChannelId::from_uuid(header.channel_id),
            PriceListId::from_uuid(header.price_list_id),
            product_variant_id,
        )
        .await?
        .ok_or_else(|| variant_unavailable(product_variant_id))?;
        if row.track_inventory {
            let available: Option<i64> = sqlx::query_scalar(
                "SELECT on_hand_quantity - reserved_quantity \
                 FROM chaos_commerce.product_variants \
                 WHERE store_id = $1 AND id = $2 AND track_inventory",
            )
            .bind(actor.store_id.as_uuid())
            .bind(product_variant_id.as_uuid())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?;
            if available.unwrap_or_default() < i64::from(quantity) {
                return Err(insufficient_inventory(product_variant_id));
            }
        }
        let line = CartLine::new(
            ProductId::from_uuid(row.product_id),
            product_variant_id,
            row.product_title,
            row.variant_title,
            row.sku,
            row.track_inventory,
            quantity,
            Money::new(row.amount_minor, currency),
        )?;
        insert_or_replace_line(&mut transaction, actor, cart_id, &line).await?;
        bump_cart(&mut transaction, actor, cart_id).await?;

        let detail = load_cart(&mut transaction, actor, cart_id)
            .await?
            .ok_or_else(|| cart_not_found(cart_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn remove_cart_line(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
        product_variant_id: ProductVariantId,
    ) -> Result<CartDetail, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        lock_active_cart(&mut transaction, actor, cart_id).await?;
        sqlx::query(
            "DELETE FROM chaos_commerce.cart_lines WHERE store_id = $1 \
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
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn create_checkout(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
        request: CheckoutRequest,
    ) -> Result<OrderId, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;

        if let Some(order_id) = existing_checkout_order_id(
            &mut transaction,
            actor,
            shopper.shopper_id,
            cart_id,
            &request,
        )
        .await?
        {
            transaction.commit().await.map_err(database_error)?;
            return Ok(order_id);
        }

        // The Cart row is the serialization boundary for checkout creation.
        // A concurrent request can observe the Order only after this lock is
        // released, so the second request must re-read it instead of creating
        // another Order or inventory reservation.
        let header = lock_cart(&mut transaction, actor, cart_id).await?;
        if header.status != "active" {
            if let Some(order_id) = existing_checkout_order_id(
                &mut transaction,
                actor,
                shopper.shopper_id,
                cart_id,
                &request,
            )
            .await?
            {
                transaction.commit().await.map_err(database_error)?;
                return Ok(order_id);
            }
            return Err(cart_not_active());
        }
        if header.channel_id != channel_id.as_uuid() {
            return Err(cart_not_found(cart_id));
        }
        let currency = parse_currency(&header.currency)?;

        require_price_list_active(
            &mut transaction,
            actor,
            PriceListId::from_uuid(header.price_list_id),
            currency,
            request.now,
        )
        .await?;
        let lines = refresh_cart_lines(
            &mut transaction,
            actor,
            cart_id,
            channel_id,
            PriceListId::from_uuid(header.price_list_id),
            currency,
        )
        .await?;
        if lines.is_empty() {
            return Err(cart_line_unavailable());
        }
        let existing_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM chaos_commerce.cart_lines WHERE store_id = $1 AND cart_id = $2",
        )
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if usize::try_from(existing_count).ok() != Some(lines.len()) {
            return Err(cart_line_unavailable());
        }
        let mut cart = Cart::rehydrate(
            cart_id,
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(header.price_list_id),
            currency,
            CartStatus::Active,
            lines.clone(),
        )?;
        cart.begin_checkout()?;
        let requested_order_id = OrderId::new();
        let subtotal = cart.total()?.amount_minor();
        let request_fingerprint = checkout_request_fingerprint(actor, &request);

        let payment_provider_account_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM chaos_integration.provider_accounts \
             WHERE store_id = $1 \
               AND capability = 'payment' \
               AND provider = $2 \
               AND enabled \
               AND credential_secret_reference IS NOT NULL \
               AND webhook_secret_reference IS NOT NULL",
        )
        .bind(actor.store_id.as_uuid())
        .bind(request.payment_provider.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(payment_provider_unavailable)?;
        // Freeze the Cart and record idempotency before reserving stock and
        // creating the Order; the transaction rolls the complete handoff back.
        let cart_locked = sqlx::query(
            "UPDATE chaos_commerce.carts SET status = 'locked'::chaos_commerce.cart_status, \
                    updated_at = $3, attribution = $4, \
                    checkout_idempotency_key = $5, checkout_request_fingerprint = $6 \
             WHERE store_id = $1 AND id = $2 AND status = 'active'",
        )
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .bind(request.now)
        .bind(&request.attribution)
        .bind(request.idempotency_key)
        .bind(request_fingerprint.as_slice())
        .execute(&mut *transaction)
        .await
        .map_err(checkout_insert_error)?
        .rows_affected();
        if cart_locked != 1 {
            return Err(cart_not_active());
        }
        reserve_inventory_for_cart(&mut transaction, actor, &cart).await?;
        // Retry random Order-number collisions without aborting the transaction.
        let mut order_number = generate_order_number()?;
        let mut attempt = 0;
        let order_created = loop {
            let inserted = sqlx::query(
                "INSERT INTO chaos_commerce.orders \
                 (id, store_id, order_number, channel_id, cart_id, shopper_id, \
                  currency, payment_provider_account_id, contact_email, \
                 subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                 shipping_amount_minor, total_amount_minor, created_at, updated_at) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,0,0,0,0,$11,$11) \
                 ON CONFLICT ON CONSTRAINT orders_store_id_order_number_key DO NOTHING",
            )
            .bind(requested_order_id.as_uuid())
            .bind(actor.store_id.as_uuid())
            .bind(order_number.as_str())
            .bind(channel_id.as_uuid())
            .bind(cart_id.as_uuid())
            .bind(shopper.shopper_id.as_uuid())
            .bind(currency.as_str())
            .bind(payment_provider_account_id)
            .bind(None::<&str>)
            .bind(subtotal)
            .bind(request.now)
            .execute(&mut *transaction)
            .await
            .map_err(checkout_insert_error)?
            .rows_affected();
            if inserted == 1 {
                break true;
            }
            attempt += 1;
            if attempt >= 5 {
                break false;
            }
            order_number = generate_order_number()?;
        };
        if !order_created {
            return Err(order_number_unavailable());
        }
        let order_id = requested_order_id;
        insert_order_lines(&mut transaction, actor, order_id, &cart, request.now).await?;

        transaction.commit().await.map_err(database_error)?;
        Ok(order_id)
    }

    /// Resolves an Order from its printed number plus the contact email on the
    /// Order, scoped to the caller's Store and Sales Channel. Malformed input,
    /// an unknown number, and a number whose email does not match all return
    /// `Ok(None)` on the same path so the lookup cannot confirm which Order
    /// numbers exist.
    pub(crate) async fn lookup_order(
        &self,
        actor: &MachineActor,
        order_number: &str,
        email: &str,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        let Ok(order_number) = OrderNumber::parse(order_number) else {
            return Ok(None);
        };
        let email = email.trim().to_lowercase();
        if email.is_empty() {
            return Ok(None);
        }
        let mut transaction = self.begin(actor).await?;
        let order_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT order_row.id \
             FROM chaos_commerce.orders AS order_row \
             WHERE order_row.store_id = $1 \
               AND order_row.order_number = $2 \
               AND order_row.contact_email = $3 \
               AND ($4::uuid IS NULL OR order_row.channel_id = $4)",
        )
        .bind(actor.store_id.as_uuid())
        .bind(order_number.as_str())
        .bind(&email)
        .bind(actor.channel_id.map(SalesChannelId::as_uuid))
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

    pub(crate) async fn get_shopper_order(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin(actor).await?;
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM chaos_commerce.orders AS order_row \
             WHERE order_row.store_id = $1 AND order_row.channel_id = $2 \
               AND order_row.shopper_id = $3 AND order_row.id = $4)",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.channel_id.map(SalesChannelId::as_uuid))
        .bind(shopper.shopper_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let order = if owned {
            load_order(&mut transaction, actor, order_id).await?
        } else {
            None
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(order)
    }

    pub(crate) async fn get_checkout_order(
        &self,
        actor: &CheckoutActor,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        let mut transaction = self.begin(actor.machine()).await?;
        let order = load_order(&mut transaction, actor.machine(), actor.order_id()).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(order)
    }
}

async fn existing_checkout_order_id(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    shopper_id: ShopperId,
    cart_id: CartId,
    request: &CheckoutRequest,
) -> Result<Option<OrderId>, ApplicationError> {
    let row = sqlx::query_as::<_, ExistingCheckoutRow>(
        "SELECT sales_order.id AS order_id, sales_order.status::text AS order_status, \
                cart.checkout_idempotency_key AS idempotency_key, \
                sales_order.payment_status::text AS payment_status, \
                cart.checkout_request_fingerprint AS request_fingerprint \
         FROM chaos_commerce.orders AS sales_order \
         INNER JOIN chaos_commerce.carts AS cart \
           ON cart.store_id = sales_order.store_id AND cart.id = sales_order.cart_id \
         WHERE sales_order.store_id = $1 AND sales_order.channel_id = $2 \
           AND sales_order.shopper_id = $3 AND sales_order.cart_id = $4",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.channel_id.map(SalesChannelId::as_uuid))
    .bind(shopper_id.as_uuid())
    .bind(cart_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };

    let requested_fingerprint = checkout_request_fingerprint(actor, request);
    ensure_checkout_request_matches(&row, request.idempotency_key, &requested_fingerprint)?;
    if row.order_status != "pending" || row.payment_status != "pending" {
        return Err(checkout_cart_already_started());
    }
    Ok(Some(OrderId::from_uuid(row.order_id)))
}

fn ensure_checkout_request_matches(
    checkout: &ExistingCheckoutRow,
    idempotency_key: Uuid,
    request_fingerprint: &[u8; 32],
) -> Result<(), ApplicationError> {
    match checkout.request_fingerprint.as_deref() {
        // A checkout belongs to its Cart and can be recovered with a freshly
        // minted HTTP idempotency key after a page reload. The fingerprint
        // preserves the original provider and return URL contract.
        Some(stored) if stored == request_fingerprint => Ok(()),
        Some(_) if checkout.idempotency_key == Some(idempotency_key) => {
            Err(idempotency_key_reused())
        }
        Some(_) => Err(checkout_cart_already_started()),
        // Compatibility for rows created before request fingerprints existed.
        None if checkout.idempotency_key == Some(idempotency_key) => Ok(()),
        None => Err(checkout_cart_already_started()),
    }
}

async fn insert_order_lines(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
    cart: &Cart,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    // Freeze the Cart's resolved image URL into the immutable Order line.
    let media = load_cart_media(transaction, actor, cart.lines()).await?;
    for (position, line) in cart.lines().iter().enumerate() {
        let subtotal = line.subtotal()?;
        let image_url = media
            .get(&(
                line.product_id().as_uuid(),
                line.product_variant_id().as_uuid(),
            ))
            .and_then(|assets| {
                assets
                    .iter()
                    .find(|asset| asset.kind == chaos_domain::catalog::MediaKind::Image)
            })
            .map(|asset| asset.url.as_str());
        sqlx::query(
            "INSERT INTO chaos_commerce.order_lines \
             (store_id, order_id, position, product_id, product_variant_id, product_title, \
              variant_title, sku, track_inventory, quantity, \
              unit_price_amount_minor, subtotal_amount_minor, image_url, created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(i16::try_from(position).map_err(unexpected_conversion)?)
        .bind(line.product_id().as_uuid())
        .bind(line.product_variant_id().as_uuid())
        .bind(line.product_title())
        .bind(line.variant_title())
        .bind(line.sku())
        .bind(line.track_inventory())
        .bind(i32::try_from(line.quantity()).map_err(unexpected_conversion)?)
        .bind(line.unit_price().amount_minor())
        .bind(subtotal.amount_minor())
        .bind(image_url)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn checkout(
        idempotency_key: Uuid,
        request_fingerprint: Option<Vec<u8>>,
    ) -> ExistingCheckoutRow {
        ExistingCheckoutRow {
            order_id: uuid(1),
            order_status: "pending".into(),
            idempotency_key: Some(idempotency_key),
            payment_status: "pending".into(),
            request_fingerprint,
        }
    }

    #[test]
    fn checkout_can_be_recovered_with_a_new_http_idempotency_key() {
        let original_key = uuid(2);
        let fingerprint = [7; 32];
        let existing = checkout(original_key, Some(fingerprint.to_vec()));

        assert!(ensure_checkout_request_matches(&existing, uuid(3), &fingerprint).is_ok());
    }

    #[test]
    fn reusing_an_idempotency_key_with_different_parameters_is_rejected() {
        let idempotency_key = uuid(2);
        let existing = checkout(idempotency_key, Some(vec![1; 32]));

        let error = ensure_checkout_request_matches(&existing, idempotency_key, &[2; 32])
            .expect_err("different checkout parameters must be rejected");

        assert!(matches!(
            error,
            ApplicationError::Conflict {
                code: "idempotency_key_reused",
                ..
            }
        ));
    }

    #[test]
    fn a_different_checkout_request_cannot_take_over_a_locked_cart() {
        let existing = checkout(uuid(2), Some(vec![1; 32]));

        let error = ensure_checkout_request_matches(&existing, uuid(3), &[2; 32])
            .expect_err("a locked Cart must retain its original checkout request");

        assert!(matches!(
            error,
            ApplicationError::Conflict {
                code: "checkout_cart_already_started",
                ..
            }
        ));
    }
}
