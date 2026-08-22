// Checkout pricing, snapshots, inventory reservation, addresses, taxes, shipping, and promotions.

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
             FROM commerce.inventory_items AS stock \
             INNER JOIN commerce.inventory_locations AS location \
               ON location.store_id = stock.store_id AND location.id = stock.inventory_location_id \
             WHERE stock.store_id = $1 \
               AND stock.product_variant_id = $2 AND location.archived_at IS NULL \
               AND stock.on_hand_quantity > stock.reserved_quantity \
             ORDER BY stock.id ASC FOR UPDATE OF stock",
        )
        .bind(actor.store_id.as_uuid())
        .bind(line.product_variant_id().as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let mut remaining = i64::from(line.quantity());
        for (inventory_item_id, on_hand, reserved) in stocks {
            if remaining == 0 {
                break;
            }
            let current = InventoryBalance::new(on_hand, reserved)?;
            let allocated = remaining.min(current.available());
            if allocated == 0 {
                continue;
            }
            let balance = current.reserve(allocated)?;
            sqlx::query(
                "UPDATE commerce.inventory_items SET reserved_quantity = $1, \
                        updated_at = CURRENT_TIMESTAMP WHERE id = $2",
            )
            .bind(balance.reserved())
            .bind(inventory_item_id)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO commerce.inventory_reservation_lines \
                 (store_id, reservation_id, inventory_item_id, quantity) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(actor.store_id.as_uuid())
            .bind(reservation_id.as_uuid())
            .bind(inventory_item_id)
            .bind(allocated)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO commerce.inventory_transactions \
                 (id, store_id, inventory_item_id, reference_type, reference_id, \
                  on_hand_delta_quantity, reserved_delta_quantity, resulting_on_hand_quantity, \
                  resulting_reserved_quantity) \
                 VALUES ($1, $2, $3, 'reservation', $4, 0, $5, $6, $7)",
            )
            .bind(Uuid::now_v7())
            .bind(actor.store_id.as_uuid())
            .bind(inventory_item_id)
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
