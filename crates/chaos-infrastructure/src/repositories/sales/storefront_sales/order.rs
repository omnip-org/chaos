// Order line materialization and inline customer snapshot reconstruction.

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
    })
}

#[derive(sqlx::FromRow)]
struct OrderIdentityRow {
    contact_email: Option<String>,
    contact_phone: Option<String>,
    billing_full_name: Option<String>,
    billing_company: Option<String>,
    billing_address_line1: Option<String>,
    billing_address_line2: Option<String>,
    billing_locality: Option<String>,
    billing_administrative_area: Option<String>,
    billing_postal_code: Option<String>,
    billing_country_code: Option<String>,
    shipping_full_name: Option<String>,
    shipping_company: Option<String>,
    shipping_address_line1: Option<String>,
    shipping_address_line2: Option<String>,
    shipping_locality: Option<String>,
    shipping_administrative_area: Option<String>,
    shipping_postal_code: Option<String>,
    shipping_country_code: Option<String>,
}

async fn load_order_identity(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<OrderIdentity, ApplicationError> {
    let row = sqlx::query_as::<_, OrderIdentityRow>(
        "SELECT contact_email::text AS contact_email, contact_phone, \
                billing_full_name, billing_company, billing_address_line1, billing_address_line2, \
                billing_locality, billing_administrative_area, billing_postal_code, \
                billing_country_code::text, shipping_full_name, shipping_company, \
                shipping_address_line1, shipping_address_line2, shipping_locality, \
                shipping_administrative_area, shipping_postal_code, shipping_country_code::text \
         FROM commerce.orders WHERE store_id = $1 AND id = $2",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_sales_state)?;
    let email = row.contact_email.ok_or_else(corrupt_sales_state)?;
    Ok(OrderIdentity::new(
        OrderContact::new(email, row.contact_phone)?,
        optional_address(
            row.billing_full_name,
            row.billing_company,
            row.billing_address_line1,
            row.billing_address_line2,
            row.billing_locality,
            row.billing_administrative_area,
            row.billing_postal_code,
            row.billing_country_code,
        )?,
        optional_address(
            row.shipping_full_name,
            row.shipping_company,
            row.shipping_address_line1,
            row.shipping_address_line2,
            row.shipping_locality,
            row.shipping_administrative_area,
            row.shipping_postal_code,
            row.shipping_country_code,
        )?,
    ))
}

#[allow(clippy::too_many_arguments)]
fn optional_address(
    full_name: Option<String>,
    company: Option<String>,
    address_line1: Option<String>,
    address_line2: Option<String>,
    locality: Option<String>,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: Option<String>,
) -> Result<Option<PostalAddress>, ApplicationError> {
    let any = full_name.is_some()
        || company.is_some()
        || address_line1.is_some()
        || address_line2.is_some()
        || locality.is_some()
        || administrative_area.is_some()
        || postal_code.is_some()
        || country_code.is_some();
    match (full_name, address_line1, locality, country_code) {
        (None, None, None, None) if !any => Ok(None),
        (Some(full_name), Some(address_line1), Some(locality), Some(country_code)) => Ok(Some(
            PostalAddress::new(
                full_name,
                company,
                address_line1,
                address_line2,
                locality,
                administrative_area,
                postal_code,
                country_code,
            )?,
        )),
        _ => Err(corrupt_sales_state()),
    }
}

pub(super) async fn load_order(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, OrderHeaderRow>(
        "SELECT id, shopper_id, price_list_id, currency::text AS currency, \
                status::text AS status, payment_status::text AS payment_status, \
                shipping_status::text AS shipping_status, subtotal_amount_minor, \
                discount_amount_minor, tax_amount_minor, \
                shipping_amount_minor, total_amount_minor, refunded_amount_minor, \
                stripe_checkout_session_id, stripe_payment_intent_id, stripe_charge_id, \
                shipping_provider, shipping_provider_reference, shipping_tracking_number, \
                shipping_tracking_url, created_at, updated_at \
         FROM commerce.orders WHERE store_id = $1 AND sales_channel_id = $2 AND id = $3",
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
    let order_number: String = sqlx::query_scalar(
        "SELECT order_number FROM commerce.orders WHERE store_id=$1 AND id=$2",
        )
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?;
    let identity = load_order_identity(transaction, actor, order_id).await?;
    let lines = sqlx::query_as::<_, OrderLineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor, \
                subtotal_amount_minor FROM commerce.order_lines \
         WHERE store_id = $1 AND order_id = $2 ORDER BY position",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let transitions = sqlx::query_as::<
        _,
        (Uuid, Option<String>, String, String, Option<Uuid>, OffsetDateTime),
    >(
        "SELECT id, from_status::text, to_status::text, kind::text, actor_user_id, occurred_at \
         FROM commerce.order_transitions WHERE store_id = $1 AND order_id = $2 \
         ORDER BY occurred_at, id",
    )
    .bind(actor.store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(Some(OrderDetail {
        id: OrderId::from_uuid(row.id),
        order_number: OrderNumber::parse(order_number)?,
        shopper_id: ShopperId::from_uuid(row.shopper_id),
        price_list_id: PriceListId::from_uuid(row.price_list_id),
        currency: parse_currency(&row.currency)?,
        status: OrderStatus::parse(&row.status).ok_or_else(corrupt_sales_state)?,
        payment_status: OrderPaymentStatus::parse(&row.payment_status)
            .ok_or_else(corrupt_sales_state)?,
        shipping_status: OrderShippingStatus::parse(&row.shipping_status)
            .ok_or_else(corrupt_sales_state)?,
        identity,
        subtotal_amount_minor: row.subtotal_amount_minor,
        discount_amount_minor: row.discount_amount_minor,
        tax_amount_minor: row.tax_amount_minor,
        shipping_amount_minor: row.shipping_amount_minor,
        total_amount_minor: row.total_amount_minor,
        refunded_amount_minor: row.refunded_amount_minor,
        stripe_checkout_session_id: row.stripe_checkout_session_id,
        stripe_payment_intent_id: row.stripe_payment_intent_id,
        stripe_charge_id: row.stripe_charge_id,
        shipping_provider: row.shipping_provider,
        shipping_provider_reference: row.shipping_provider_reference,
        shipping_tracking_number: row.shipping_tracking_number,
        shipping_tracking_url: row.shipping_tracking_url,
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
                        Some(status) => Some(
                            OrderStatus::parse(status).ok_or_else(corrupt_sales_state)?,
                        ),
                        None => None,
                    },
                    to_status: OrderStatus::parse(&value.2).ok_or_else(corrupt_sales_state)?,
                    kind: value.3,
                    actor_user_id: value.4,
                    occurred_at: value.5,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}
