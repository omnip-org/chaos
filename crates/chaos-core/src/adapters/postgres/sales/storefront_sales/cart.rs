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
         INNER JOIN commerce.store_sales_channels AS channel \
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
) -> Result<Option<(Uuid, String, String, Option<String>, bool, bool, i64)>, ApplicationError>
{
    sqlx::query_as(
        "SELECT product.id, product.title, variant.title, variant.sku::text, \
                variant.requires_shipping, variant.track_inventory, price.amount_minor \
         FROM commerce.product_variants AS variant \
         INNER JOIN commerce.products AS product \
           ON product.store_id = variant.store_id AND product.id = variant.product_id \
         INNER JOIN commerce.product_publications AS publication \
           ON publication.store_id = product.store_id AND publication.product_id = product.id \
          AND publication.sales_channel_id = $1 \
         INNER JOIN commerce.price_lists AS price_list \
           ON price_list.store_id = variant.store_id AND price_list.id = $2 \
         INNER JOIN commerce.prices AS price \
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
         (store_id, cart_id, product_id, product_variant_id, \
          product_title, variant_title, sku, requires_shipping, track_inventory, quantity, \
          unit_price_amount_minor) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         ON CONFLICT (store_id, cart_id, product_variant_id) \
         DO UPDATE SET product_title = EXCLUDED.product_title, \
             variant_title = EXCLUDED.variant_title, sku = EXCLUDED.sku, \
             requires_shipping = EXCLUDED.requires_shipping, \
             track_inventory = EXCLUDED.track_inventory, quantity = EXCLUDED.quantity, \
             unit_price_amount_minor = EXCLUDED.unit_price_amount_minor, \
             updated_at = CURRENT_TIMESTAMP",
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
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
        "SELECT cart.sales_channel_id, cart.price_list_id, price_list.currency::text, \
                cart.status::text \
         FROM commerce.carts AS cart \
         INNER JOIN commerce.price_lists AS price_list \
           ON price_list.store_id = cart.store_id AND price_list.id = cart.price_list_id \
         WHERE cart.store_id = $1 \
           AND cart.sales_channel_id = $2 AND cart.id = $3 FOR UPDATE OF cart",
    )
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
    Ok((row.0, row.1, row.2, row.3))
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
           AND cart.sales_channel_id = $2 AND cart.id = $3",
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
    let status = CartStatus::parse(&row.4).ok_or_else(corrupt_sales_state)?;
    let lines = load_cart_line_rows(transaction, actor, cart_id).await?;
    let media = load_cart_media(transaction, actor, &lines).await?;
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
        status,
        version: u64::try_from(row.5).map_err(unexpected_conversion)?,
        lines: items,
        subtotal_amount_minor: subtotal.amount_minor(),
        created_at: row.6,
        updated_at: row.7,
    }))
}

/// Media for the exact Product+Variant of each Cart line: a variant-specific
/// asset (`media.product_variant_id` matching this line's variant) plus every
/// product-level asset (`media.product_variant_id IS NULL`), so a line for
/// one variant never shows another variant's exclusive photos.
async fn load_cart_media(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    lines: &[CartLineRow],
) -> Result<HashMap<(Uuid, Uuid), Vec<StorefrontMediaAsset>>, ApplicationError> {
    let product_ids = lines.iter().map(|line| line.0).collect::<Vec<_>>();
    if product_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, CartMediaRow>(
        "SELECT media.product_id, media.id, media.product_variant_id, media.media_type, \
                media.media_kind::text, media.alt_text, \
                media.position, media.public_url \
         FROM commerce.media_assets AS media \
         WHERE media.store_id = $1 AND media.product_id = ANY($2) AND media.status = 'ready' \
         ORDER BY media.product_id, media.position, media.id",
    )
    .bind(actor.store_id.as_uuid())
    .bind(&product_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let mut by_product: HashMap<Uuid, Vec<StorefrontMediaAsset>> = HashMap::new();
    for row in rows {
        let kind = match row.4.as_str() {
            "image" => chaos_domain::catalog::MediaKind::Image,
            "video" => chaos_domain::catalog::MediaKind::Video,
            _ => return Err(corrupt_sales_state()),
        };
        by_product
            .entry(row.0)
            .or_default()
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
    let mut by_line = HashMap::new();
    for line in lines {
        let (product_id, product_variant_id) = (line.0, line.1);
        let scoped = by_product
            .get(&product_id)
            .map(|assets| {
                assets
                    .iter()
                    .filter(|asset| {
                        asset
                            .product_variant_id
                            .is_none_or(|id| id.as_uuid() == product_variant_id)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        by_line.insert((product_id, product_variant_id), scoped);
    }
    Ok(by_line)
}

async fn load_cart_line_rows(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
) -> Result<Vec<CartLineRow>, ApplicationError> {
    sqlx::query_as(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor \
         FROM commerce.cart_lines \
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
    media: &HashMap<(Uuid, Uuid), Vec<StorefrontMediaAsset>>,
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
        media: media.get(&(row.0, row.1)).cloned().unwrap_or_default(),
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
