// Cart-to-Order commands and the Stripe Embedded Checkout handoff.

async fn ensure_inventory_available(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart: &Cart,
) -> Result<(), ApplicationError> {
    for line in cart.lines().iter().filter(|line| line.track_inventory()) {
        let available: Option<i64> = sqlx::query_scalar(
            "SELECT on_hand_quantity \
             FROM commerce.product_variants \
             WHERE store_id = $1 AND id = $2 AND track_inventory FOR UPDATE",
        )
        .bind(actor.store_id.as_uuid())
        .bind(line.product_variant_id().as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
        if available.unwrap_or_default() < i64::from(line.quantity()) {
            return Err(insufficient_inventory(line.product_variant_id()));
        }
    }
    Ok(())
}

impl PostgresStorefrontSalesRepository {
    pub(crate) async fn create_shopper(&self, actor: &MachineActor) -> Result<ShopperId, ApplicationError> {
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

    pub(crate) async fn create_cart(
        &self,
        shopper: &ShopperActor,
        currency: Option<CurrencyCode>,
    ) -> Result<CartDetail, ApplicationError> {
        let shopper_id = shopper.shopper_id;
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let (price_list_id, currency) =
            select_price_list(&mut transaction, actor, channel_id, currency)
                .await?
                .ok_or_else(price_context_unavailable)?;
        let cart = Cart::create(
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(price_list_id),
            currency,
        );
        sqlx::query(
            "INSERT INTO commerce.carts \
             (id, store_id, shopper_id, sales_channel_id, price_list_id, currency) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(cart.id().as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(shopper_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(price_list_id)
        .bind(currency.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = load_cart(&mut transaction, actor, cart.id())
            .await?
            .ok_or_else(|| cart_not_found(cart.id()))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn get_cart(
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
        let row = resolve_variant(
            &mut transaction,
            actor,
            SalesChannelId::from_uuid(header.0),
            PriceListId::from_uuid(header.1),
            product_variant_id,
        )
        .await?
        .ok_or_else(|| variant_unavailable(product_variant_id))?;
        if row.5 {
            let available: Option<i64> = sqlx::query_scalar(
                "SELECT on_hand_quantity \
                 FROM commerce.product_variants \
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
            ProductId::from_uuid(row.0),
            product_variant_id,
            row.1,
            row.2,
            row.3,
            row.4,
            row.5,
            quantity,
            Money::new(row.6, currency),
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
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn create_stripe_checkout(
        &self,
        shopper: &ShopperActor,
        cart_id: CartId,
        email: &str,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
        request_id: Uuid,
    ) -> Result<StripeCheckoutDraft, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
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
        if lines.is_empty() {
            return Err(cart_line_unavailable());
        }
        let existing_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commerce.cart_lines WHERE store_id = $1 AND cart_id = $2",
        )
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
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(header.1),
            currency,
            CartStatus::Active,
            lines.clone(),
        )?;
        ensure_inventory_available(&mut transaction, actor, &cart).await?;
        let requested_order_id = OrderId::new();
        let subtotal = cart.total()?.amount_minor();
        let order_number = generate_order_number(now)?;
        let inserted_order_id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO commerce.orders \
             (id, store_id, order_number, sales_channel_id, cart_id, shopper_id, request_id, \
              price_list_id, currency, contact_email, \
              subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
              shipping_amount_minor, total_amount_minor, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,0,0,0,0,0,$12,$12) \
             ON CONFLICT (store_id, sales_channel_id, shopper_id, request_id) DO NOTHING \
             RETURNING id",
        )
        .bind(requested_order_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(order_number.as_str())
        .bind(channel_id.as_uuid())
        .bind(cart_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(request_id)
        .bind(header.1)
        .bind(currency.as_str())
        .bind(email)
        .bind(subtotal)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let Some(_) = inserted_order_id else {
            let existing = sqlx::query_as::<_, (Uuid, String, i64)>(
                "SELECT id, currency::text, subtotal_amount_minor FROM commerce.orders \
                 WHERE store_id = $1 AND sales_channel_id = $2 AND shopper_id = $3 \
                   AND request_id = $4",
            )
            .bind(actor.store_id.as_uuid())
            .bind(channel_id.as_uuid())
            .bind(shopper.shopper_id.as_uuid())
            .bind(request_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(StripeCheckoutDraft {
                order_id: OrderId::from_uuid(existing.0),
                currency: parse_currency(&existing.1)?,
                subtotal_amount_minor: existing.2,
                expires_at,
            });
        };
        let order_id = requested_order_id;
        insert_order_lines(&mut transaction, actor, order_id, &cart, now).await?;
        sqlx::query(
            "INSERT INTO commerce.order_transitions \
             (id, store_id, order_id, from_status, to_status, kind, occurred_at) \
             VALUES ($1, $2, $3, NULL, 'pending', 'created', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        append_event(
            &mut transaction,
            AnalyticsEventToAppend {
                store_id: actor.store_id.as_uuid(),
                shopper_id: shopper.shopper_id.as_uuid(),
                event_id: order_id.as_uuid(),
                event_name: "initiate_checkout".into(),
                properties: json!({
                    "_source": "server",
                    "cart_id": cart_id.as_uuid(),
                    "order_id": order_id.as_uuid(),
                    "payment_ui": "stripe_embedded_checkout",
                }),
                occurred_at: now,
                received_at: now,
            },
        )
        .await?;
        let draft = StripeCheckoutDraft {
            order_id,
            currency,
            subtotal_amount_minor: subtotal,
            expires_at,
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(draft)
    }

    pub(crate) async fn get_order(
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

    pub(crate) async fn get_tracked_order(
        &self,
        actor: &MachineActor,
        tracking_token: &SecretString,
        now: OffsetDateTime,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        if !valid_capability(tracking_token.expose_secret(), "ot_") {
            return Ok(None);
        }
        let mut transaction = self.begin(actor).await?;
        let digest: [u8; 32] = Sha256::digest(tracking_token.expose_secret()).into();
        let order_id: Option<Uuid> = sqlx::query_scalar(
            "UPDATE commerce.order_tracking_tokens AS token SET last_used_at=$1 \
             WHERE token.store_id=$2 AND token.token_digest=$3 AND token.expires_at>$1 \
               AND EXISTS ( \
                   SELECT 1 FROM commerce.orders AS order_row \
                   WHERE order_row.store_id=token.store_id \
                     AND order_row.id=token.order_id \
                     AND order_row.sales_channel_id=$4 \
               ) \
             RETURNING token.order_id",
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

async fn insert_order_lines(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
    cart: &Cart,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    for (position, line) in cart.lines().iter().enumerate() {
        let subtotal = line.subtotal()?;
        sqlx::query(
            "INSERT INTO commerce.order_lines \
             (store_id, order_id, position, product_id, product_variant_id, product_title, \
              variant_title, sku, requires_shipping, track_inventory, quantity, \
              unit_price_amount_minor, subtotal_amount_minor, created_at) \
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
        .bind(line.requires_shipping())
        .bind(line.track_inventory())
        .bind(i32::try_from(line.quantity()).map_err(unexpected_conversion)?)
        .bind(line.unit_price().amount_minor())
        .bind(subtotal.amount_minor())
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

fn valid_capability(value: &str, prefix: &str) -> bool {
    value.len() == prefix.len() + 43
        && value.starts_with(prefix)
        && value[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}
