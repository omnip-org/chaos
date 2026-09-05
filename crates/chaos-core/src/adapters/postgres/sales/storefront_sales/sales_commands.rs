// Cart-to-Order commands and the Stripe Embedded Checkout handoff.

async fn reserve_inventory_for_cart(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart: &Cart,
) -> Result<(), ApplicationError> {
    for line in cart.lines().iter().filter(|line| line.track_inventory()) {
        let reserved: Option<Uuid> = sqlx::query_scalar(
            "UPDATE commerce.product_variants \
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
    ) -> Result<CartDetail, ApplicationError> {
        let shopper_id = shopper.shopper_id;
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;

        // Cart creation is an idempotent session operation. The partial
        // unique index is the database guard; this read also makes repeated
        // requests return the canonical active Cart without minting another.
        if let Some(cart_id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM commerce.carts \
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
            "INSERT INTO commerce.carts \
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
            "SELECT id FROM commerce.carts \
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
        expected_version: u64,
    ) -> Result<CartDetail, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        let header = lock_active_cart(&mut transaction, actor, cart_id).await?;
        ensure_cart_version(header.4, expected_version)?;
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
        if row.4 {
            let available: Option<i64> = sqlx::query_scalar(
                "SELECT on_hand_quantity - reserved_quantity \
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
            quantity,
            Money::new(row.5, currency),
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
        expected_version: u64,
    ) -> Result<CartDetail, ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;
        let header = lock_active_cart(&mut transaction, actor, cart_id).await?;
        ensure_cart_version(header.4, expected_version)?;
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
        request: StripeCheckoutRequest,
    ) -> Result<CheckoutDraft, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = require_channel(actor)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        ensure_cart_owner(&mut transaction, actor, cart_id, shopper.shopper_id).await?;

        if let Some(draft) = existing_checkout_draft(
            &mut transaction,
            actor,
            shopper.shopper_id,
            cart_id,
            &request,
        )
        .await?
        {
            transaction.commit().await.map_err(database_error)?;
            return Ok(draft);
        }

        // The Cart row is the serialization boundary for checkout creation.
        // A concurrent request can observe the Order only after this lock is
        // released, so the second request must re-read it instead of creating
        // another Order or inventory reservation.
        let header = lock_cart(&mut transaction, actor, cart_id).await?;
        if header.3 != "active" {
            if let Some(draft) = existing_checkout_draft(
                &mut transaction,
                actor,
                shopper.shopper_id,
                cart_id,
                &request,
            )
            .await?
            {
                transaction.commit().await.map_err(database_error)?;
                return Ok(draft);
            }
            return Err(cart_not_active());
        }
        if header.0 != channel_id.as_uuid() {
            return Err(cart_not_found(cart_id));
        }
        let currency = parse_currency(&header.2)?;

        require_price_list_active(
            &mut transaction,
            actor,
            PriceListId::from_uuid(header.1),
            currency,
            request.now,
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
        let mut cart = Cart::rehydrate(
            cart_id,
            actor.store_id,
            channel_id,
            PriceListId::from_uuid(header.1),
            currency,
            CartStatus::Active,
            lines.clone(),
        )?;
        cart.begin_checkout()?;
        let requested_order_id = OrderId::new();
        let subtotal = cart.total()?.amount_minor();
        let request_fingerprint = checkout_request_fingerprint(actor, &request);

        let order_number = generate_order_number(request.now)?;
        let payment_provider_account_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM integration.provider_accounts \
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
        // The checkout transaction owns the complete handoff: freeze the Cart,
        // reserve stock, create the pending Order, and persist its immutable
        // line snapshot. Any later failure rolls the whole handoff back.
        let cart_locked = sqlx::query(
            "UPDATE commerce.carts SET status = 'locked'::commerce.cart_status, \
                    version = version + 1, updated_at = $3, attribution = $4 \
             WHERE store_id = $1 AND id = $2 AND status = 'active'",
        )
        .bind(actor.store_id.as_uuid())
        .bind(cart_id.as_uuid())
        .bind(request.now)
        .bind(&request.attribution)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        if cart_locked != 1 {
            return Err(cart_not_active());
        }
        reserve_inventory_for_cart(&mut transaction, actor, &cart).await?;
        sqlx::query(
            "INSERT INTO commerce.orders \
             (id, store_id, order_number, channel_id, cart_id, shopper_id, idempotency_key, \
              price_list_id, currency, payment_provider_account_id, contact_email, \
             checkout_request_fingerprint, \
             subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
             shipping_amount_minor, total_amount_minor, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,0,0,0,0,$14,$14)",
        )
        .bind(requested_order_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(order_number.as_str())
        .bind(channel_id.as_uuid())
        .bind(cart_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(request.idempotency_key)
        .bind(header.1)
        .bind(currency.as_str())
        .bind(payment_provider_account_id)
        .bind(None::<&str>)
        .bind(request_fingerprint.as_slice())
        .bind(subtotal)
        .bind(request.now)
        .execute(&mut *transaction)
        .await
        .map_err(checkout_insert_error)?;
        let order_id = requested_order_id;
        insert_order_lines(&mut transaction, actor, order_id, &cart, request.now).await?;

        // Reuses the Order id as the Meta CAPI event id (same convention as
        // Purchase, see `payments/events.rs`): stable across a checkout
        // retry for free, since a retry always resolves the same Order
        // through `existing_checkout_draft` above rather than reaching here
        // again, and it's what the browser Pixel projection reuses to
        // dedup its own InitiateCheckout against this one.
        let items: Vec<Value> = cart
            .lines()
            .iter()
            .map(|line| {
                json!({
                    "product_id": line.product_id().as_uuid(),
                    "product_variant_id": line.product_variant_id().as_uuid(),
                    "quantity": i64::from(line.quantity()),
                    "price_minor": line.unit_price().amount_minor(),
                })
            })
            .collect();
        let mut initiate_checkout_properties = json!({
            "_source": "server",
            "order_id": order_id.as_uuid(),
            "value_minor": subtotal,
            "currency": currency.as_str(),
            "items": items,
        });
        if let Some(attribution) = &request.attribution {
            splice_attribution(&mut initiate_checkout_properties, attribution);
        }
        append_event(
            &mut transaction,
            AnalyticsEventToAppend {
                store_id: actor.store_id.as_uuid(),
                channel_id: channel_id.as_uuid(),
                shopper_id: shopper.shopper_id.as_uuid(),
                event_id: order_id.as_uuid(),
                event_name: "initiate_checkout".into(),
                event_source: "server",
                properties: initiate_checkout_properties,
                occurred_at: request.now,
                received_at: request.now,
            },
        )
        .await?;

        let draft = CheckoutDraft {
            order_id,
            source_cart_id: cart_id,
            currency,
            subtotal_amount_minor: subtotal,
            event_id: order_id.as_uuid(),
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(draft)
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
             FROM commerce.orders AS order_row \
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
}

async fn existing_checkout_draft(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    shopper_id: ShopperId,
    cart_id: CartId,
    request: &StripeCheckoutRequest,
) -> Result<Option<CheckoutDraft>, ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Uuid,
            String,
            String,
            i64,
            Option<Vec<u8>>,
        ),
    >(
        "SELECT sales_order.id, sales_order.status::text, \
                sales_order.idempotency_key, sales_order.payment_status::text, \
                sales_order.currency::text, sales_order.subtotal_amount_minor, \
                sales_order.checkout_request_fingerprint \
         FROM commerce.orders AS sales_order \
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

    if row.2 != request.idempotency_key {
        return Err(checkout_cart_already_started());
    }
    if let Some(stored_fingerprint) = row.6 {
        let requested_fingerprint = checkout_request_fingerprint(actor, request);
        if stored_fingerprint.as_slice() != requested_fingerprint.as_slice() {
            return Err(idempotency_key_reused());
        }
    }
    if row.1 != "pending" || row.3 != "pending" {
        return Err(checkout_cart_already_started());
    }
    Ok(Some(CheckoutDraft {
        order_id: OrderId::from_uuid(row.0),
        source_cart_id: cart_id,
        currency: parse_currency(&row.4)?,
        subtotal_amount_minor: row.5,
        event_id: row.0,
    }))
}

fn ensure_cart_version(current: i64, expected: u64) -> Result<(), ApplicationError> {
    if u64::try_from(current).ok() == Some(expected) {
        Ok(())
    } else {
        Err(ApplicationError::Conflict {
            code: "cart_version_conflict",
            message: "the Cart changed; reload it before retrying the mutation",
        })
    }
}

async fn insert_order_lines(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
    cart: &Cart,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    // Snapshot the presentation image the shopper saw for each line. Resolution
    // follows the same exact Variant -> Option Value -> Product fallback as the
    // Cart read; the URL is frozen here because Order lines are immutable history
    // and the Worker builds the Stripe session from this table alone.
    let media = load_cart_media(transaction, actor, cart.lines()).await?;
    for (position, line) in cart.lines().iter().enumerate() {
        let subtotal = line.subtotal()?;
        let image_url = media
            .get(&(line.product_id().as_uuid(), line.product_variant_id().as_uuid()))
            .and_then(|assets| {
                assets
                    .iter()
                    .find(|asset| asset.kind == chaos_domain::catalog::MediaKind::Image)
            })
            .map(|asset| asset.url.as_str());
        sqlx::query(
            "INSERT INTO commerce.order_lines \
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
