// Cart product resolution, pricing context, line management, and cart reads.

/// Resolves the Store's single active Price List for a Sales Channel. A
/// Store trades in exactly one currency (`stores.currency`), so a Cart or
/// Order never chooses a currency — it inherits whichever Price List is
/// active, and every Price List row already carries the Store's currency.
async fn select_price_list(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    channel_id: SalesChannelId,
) -> Result<Option<(Uuid, CurrencyCode)>, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT price_list.id, price_list.currency::text \
         FROM commerce.price_lists AS price_list \
         INNER JOIN commerce.stores AS store \
           ON store.id = price_list.store_id \
         INNER JOIN commerce.channels AS channel \
           ON channel.store_id = store.id AND channel.id = $1 \
         WHERE price_list.store_id = $2 \
           AND store.status = 'active' AND channel.status = 'active' \
           AND price_list.status = 'active' \
           AND price_list.currency = store.currency \
           AND (price_list.starts_at IS NULL OR price_list.starts_at <= CURRENT_TIMESTAMP) \
           AND (price_list.ends_at IS NULL OR price_list.ends_at > CURRENT_TIMESTAMP) \
         ORDER BY price_list.starts_at DESC NULLS LAST, price_list.id ASC LIMIT 1",
    )
    .bind(channel_id.as_uuid())
    .bind(actor.store_id.as_uuid())
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
) -> Result<Option<(Uuid, String, String, Option<String>, bool, i64)>, ApplicationError> {
    sqlx::query_as(
        "SELECT product.id, product.title, variant.title, variant.sku::text, \
                variant.track_inventory, price.amount_minor \
         FROM commerce.product_variants AS variant \
         INNER JOIN commerce.products AS product \
           ON product.store_id = variant.store_id AND product.id = variant.product_id \
         INNER JOIN commerce.product_publications AS publication \
           ON publication.store_id = product.store_id AND publication.product_id = product.id \
          AND publication.channel_id = $1 \
         INNER JOIN commerce.price_lists AS price_list \
           ON price_list.store_id = variant.store_id AND price_list.id = $2 \
         INNER JOIN commerce.price_list_items AS price \
           ON price.store_id = variant.store_id AND price.price_list_id = price_list.id \
          AND price.product_variant_id = variant.id \
         WHERE variant.store_id = $3 AND variant.id = $4 \
           AND variant.status = 'active' AND product.status = 'active' \
           AND price_list.status = 'active' \
           AND (price_list.starts_at IS NULL OR price_list.starts_at <= CURRENT_TIMESTAMP) \
           AND (price_list.ends_at IS NULL OR price_list.ends_at > CURRENT_TIMESTAMP)",
    )
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
         (store_id, cart_id, product_variant_id, quantity) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (store_id, cart_id, product_variant_id) \
         DO UPDATE SET quantity = EXCLUDED.quantity, \
             updated_at = CURRENT_TIMESTAMP",
    )
    .bind(actor.store_id.as_uuid())
    .bind(cart_id.as_uuid())
    .bind(line.product_variant_id().as_uuid())
    .bind(i32::try_from(line.quantity()).map_err(unexpected_conversion)?)
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
) -> Result<(Uuid, Uuid, String, String, i64), ApplicationError> {
    let row = lock_cart(transaction, actor, cart_id).await?;
    if row.3 != "active" {
        return Err(cart_not_active());
    }
    Ok(row)
}

async fn lock_cart(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<(Uuid, Uuid, String, String, i64), ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, i64)>(
        "SELECT cart.channel_id, cart.price_list_id, price_list.currency::text, \
                cart.status::text, cart.version \
         FROM commerce.carts AS cart \
         INNER JOIN commerce.price_lists AS price_list \
           ON price_list.store_id = cart.store_id AND price_list.id = cart.price_list_id \
         WHERE cart.store_id = $1 \
           AND cart.channel_id = $2 AND cart.id = $3 FOR UPDATE OF cart",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| cart_not_found(cart_id))?;
    Ok(row)
}

async fn load_cart(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<Option<CartDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, CartHeaderRow>(
        "SELECT cart.id, cart.shopper_id, cart.price_list_id, price_list.currency::text, \
                cart.status::text, cart.version, cart.created_at, cart.updated_at \
         FROM commerce.carts AS cart \
         INNER JOIN commerce.price_lists AS price_list \
           ON price_list.store_id = cart.store_id AND price_list.id = cart.price_list_id \
         WHERE cart.store_id = $1 \
           AND cart.channel_id = $2 AND cart.id = $3",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let channel_id = require_channel(actor)?;
    let currency = parse_currency(&row.3)?;
    let status = CartStatus::parse(&row.4).ok_or_else(corrupt_sales_state)?;
    let lines = refresh_cart_lines(
        transaction,
        actor,
        cart_id,
        channel_id,
        PriceListId::from_uuid(row.2),
        currency,
    )
    .await?;
    let media = load_cart_media(transaction, actor, &lines).await?;
    let items = lines
        .into_iter()
        .map(|line| cart_line_item(line, &media))
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
        status,
        version: u64::try_from(row.5).map_err(unexpected_conversion)?,
        lines: items,
        subtotal_amount_minor: subtotal.amount_minor(),
        created_at: row.6,
        updated_at: row.7,
    }))
}

/// Media for each Cart line follows the same fallback contract as the catalog:
/// exact Variant media, then media attached to one of the Variant's selected
/// Option Values, then Product media. The selected Option Values are loaded in
/// one query so a cart with many lines does not perform one media query per line.
async fn load_cart_media(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    lines: &[CartLine],
) -> Result<HashMap<(Uuid, Uuid), Vec<StorefrontMediaAsset>>, ApplicationError> {
    let product_ids = lines
        .iter()
        .map(|line| line.product_id().as_uuid())
        .collect::<Vec<_>>();
    if product_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, CartMediaRow>(
        "SELECT link.product_id, media.id, 'product'::text, NULL::uuid, NULL::uuid, NULL::uuid, \
                media.media_type, media.media_kind::text, link.alt_text, link.position, media.public_url \
         FROM commerce.product_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         WHERE link.store_id = $1 AND link.product_id = ANY($2) \
           AND link.archived_at IS NULL AND media.status = 'ready' \
         UNION ALL \
         SELECT link.product_id, media.id, 'option_value'::text, link.option_id, link.option_value_id, NULL::uuid, \
                media.media_type, media.media_kind::text, link.alt_text, link.position, media.public_url \
         FROM commerce.product_option_value_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         INNER JOIN commerce.product_options AS option \
            ON option.store_id=link.store_id AND option.product_id=link.product_id \
           AND option.id=link.option_id AND option.archived_at IS NULL \
         INNER JOIN commerce.product_option_values AS option_value \
            ON option_value.store_id=link.store_id AND option_value.product_id=link.product_id \
           AND option_value.option_id=link.option_id AND option_value.id=link.option_value_id \
           AND option_value.archived_at IS NULL \
         WHERE link.store_id = $1 AND link.product_id = ANY($2) \
           AND link.archived_at IS NULL AND media.status = 'ready' \
         UNION ALL \
         SELECT link.product_id, media.id, 'variant'::text, NULL::uuid, NULL::uuid, link.product_variant_id, \
                media.media_type, media.media_kind::text, link.alt_text, link.position, media.public_url \
         FROM commerce.product_variant_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         INNER JOIN commerce.product_variants AS variant \
            ON variant.store_id=link.store_id AND variant.product_id=link.product_id \
           AND variant.id=link.product_variant_id AND variant.status='active' \
         WHERE link.store_id = $1 AND link.product_id = ANY($2) \
           AND link.archived_at IS NULL AND media.status = 'ready' \
         ORDER BY 1, 10, 3, 2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(&product_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let mut by_product: HashMap<Uuid, Vec<StorefrontMediaAsset>> = HashMap::new();
    for row in rows {
        let kind = match row.7.as_str() {
            "image" => chaos_domain::catalog::MediaKind::Image,
            "video" => chaos_domain::catalog::MediaKind::Video,
            _ => return Err(corrupt_sales_state()),
        };
        by_product
            .entry(row.0)
            .or_default()
            .push(StorefrontMediaAsset {
                id: chaos_domain::catalog::MediaAssetId::from_uuid(row.1),
                scope: match row.2.as_str() {
                    "product" => StorefrontMediaScope::Product,
                    "option_value" => StorefrontMediaScope::OptionValue {
                        option_id: ProductOptionId::from_uuid(row.3.ok_or_else(
                            corrupt_sales_state,
                        )?),
                        option_value_id: ProductOptionValueId::from_uuid(
                            row.4.ok_or_else(corrupt_sales_state)?,
                        ),
                    },
                    "variant" => StorefrontMediaScope::Variant {
                        product_variant_id: ProductVariantId::from_uuid(
                            row.5.ok_or_else(corrupt_sales_state)?,
                        ),
                    },
                    _ => return Err(corrupt_sales_state()),
                },
                media_type: row.6,
                kind,
                alt_text: row.8,
                position: u16::try_from(row.9).map_err(unexpected_conversion)?,
                url: row.10,
            });
    }

    let variant_ids = lines
        .iter()
        .map(|line| line.product_variant_id().as_uuid())
        .collect::<Vec<_>>();
    let selected_rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        "SELECT selection.variant_id, selection.option_id, selection.option_value_id \
         FROM commerce.variant_selected_options AS selection \
         INNER JOIN commerce.product_options AS option \
           ON option.store_id=selection.store_id \
          AND option.product_id=selection.product_id \
          AND option.id=selection.option_id \
          AND option.archived_at IS NULL \
         INNER JOIN commerce.product_option_values AS option_value \
           ON option_value.store_id=selection.store_id \
          AND option_value.product_id=selection.product_id \
          AND option_value.option_id=selection.option_id \
          AND option_value.id=selection.option_value_id \
          AND option_value.archived_at IS NULL \
         INNER JOIN commerce.product_variants AS variant \
           ON variant.store_id=selection.store_id \
          AND variant.product_id=selection.product_id \
          AND variant.id=selection.variant_id \
          AND variant.status='active' \
         WHERE selection.store_id=$1 \
           AND selection.product_id=ANY($2) \
           AND selection.variant_id=ANY($3) \
         ORDER BY selection.variant_id, selection.option_id",
    )
    .bind(actor.store_id.as_uuid())
    .bind(&product_ids)
    .bind(&variant_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let mut selected_by_variant: HashMap<Uuid, Vec<StorefrontSelectedOption>> = HashMap::new();
    for (variant_id, option_id, option_value_id) in selected_rows {
        selected_by_variant
            .entry(variant_id)
            .or_default()
            .push(StorefrontSelectedOption {
                option_id: ProductOptionId::from_uuid(option_id),
                option_value_id: ProductOptionValueId::from_uuid(option_value_id),
            });
    }

    let mut by_line = HashMap::new();
    for line in lines {
        let (product_id, product_variant_id) = (
            line.product_id().as_uuid(),
            line.product_variant_id().as_uuid(),
        );
        let scoped = by_product
            .get(&product_id)
            .map(|assets| {
                resolve_storefront_media(
                    assets,
                    ProductVariantId::from_uuid(product_variant_id),
                    selected_by_variant
                        .get(&product_variant_id)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        by_line.insert((product_id, product_variant_id), scoped);
    }
    Ok(by_line)
}

fn cart_line_item(
    line: CartLine,
    media: &HashMap<(Uuid, Uuid), Vec<StorefrontMediaAsset>>,
) -> Result<CartLineItem, ApplicationError> {
    let quantity = line.quantity();
    let subtotal = line.subtotal()?;
    Ok(CartLineItem {
        product_id: line.product_id(),
        product_variant_id: line.product_variant_id(),
        product_title: line.product_title().into(),
        variant_title: line.variant_title().into(),
        sku: line.sku().map(str::to_owned),
        track_inventory: line.track_inventory(),
        quantity,
        unit_price_amount_minor: line.unit_price().amount_minor(),
        subtotal_amount_minor: subtotal.amount_minor(),
        media: media
            .get(&(line.product_id().as_uuid(), line.product_variant_id().as_uuid()))
            .cloned()
            .unwrap_or_default(),
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
        "SELECT product.id, variant.id, product.title, variant.title, \
                variant.sku::text, variant.track_inventory, cart_line.quantity, \
                price.amount_minor \
         FROM commerce.cart_lines AS cart_line \
         INNER JOIN commerce.product_variants AS variant \
           ON variant.store_id = cart_line.store_id \
          AND variant.id = cart_line.product_variant_id AND variant.status = 'active' \
         INNER JOIN commerce.products AS product \
           ON product.store_id = variant.store_id AND product.id = variant.product_id \
          AND product.status = 'active' \
         INNER JOIN commerce.product_publications AS publication \
           ON publication.store_id = product.store_id AND publication.product_id = product.id \
          AND publication.channel_id = $1 \
         INNER JOIN commerce.price_lists AS price_list \
           ON price_list.store_id = cart_line.store_id AND price_list.id = $2 \
         INNER JOIN commerce.price_list_items AS price \
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
                u32::try_from(row.6).map_err(unexpected_conversion)?,
                Money::new(row.7, currency),
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
