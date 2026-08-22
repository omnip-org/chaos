// Cart, checkout, and order idempotency snapshots and domain reconstruction.

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
