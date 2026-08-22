// Cart and Stripe payment-attempt idempotency snapshots.

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
