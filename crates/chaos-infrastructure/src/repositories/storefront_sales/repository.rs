// Cart, checkout, and order repository commands plus checkout expiry queue handling.

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
