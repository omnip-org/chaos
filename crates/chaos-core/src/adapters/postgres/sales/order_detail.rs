use std::collections::HashMap;

use crate::{
    ApplicationError,
    contracts::{
        OrderDetail, OrderFulfillmentItem, OrderLineItem, OrderPaymentAttemptItem, OrderRefundItem,
    },
    error::database_error,
};
use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    fulfillment::{FulfillmentId, FulfillmentProviderAccountId, FulfillmentStatus},
    integration::{FulfillmentProvider, PaymentProvider},
    payments::{PaymentAttemptStatus, RefundId, RefundStatus},
    pricing::PriceListId,
    sales::{
        OrderContact, OrderId, OrderIdentity, OrderNumber, OrderPaymentStatus, OrderStatus,
        PostalAddress, ShopperId,
    },
    store::{SalesChannelId, StoreId},
};
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct OrderHeaderRow {
    id: Uuid,
    order_number: String,
    shopper_id: Uuid,
    price_list_id: Uuid,
    currency: String,
    status: String,
    payment_status: String,
    fulfillment_status: String,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    amounts_finalized_at: Option<OffsetDateTime>,
    refunded_amount_minor: i64,
    payment_provider: Option<String>,
    payment_provider_reference_id: Option<String>,
    payment_failure_code: Option<String>,
    contact_email: Option<String>,
    contact_phone: Option<String>,
    billing_full_name: Option<String>,
    billing_address_line1: Option<String>,
    billing_address_line2: Option<String>,
    billing_locality: Option<String>,
    billing_administrative_area: Option<String>,
    billing_postal_code: Option<String>,
    billing_country_code: Option<String>,
    shipping_full_name: Option<String>,
    shipping_address_line1: Option<String>,
    shipping_address_line2: Option<String>,
    shipping_locality: Option<String>,
    shipping_administrative_area: Option<String>,
    shipping_postal_code: Option<String>,
    shipping_country_code: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct RefundRow {
    order_id: Uuid,
    id: Uuid,
    status: String,
    amount_minor: i64,
    provider_reference_id: Option<String>,
    failure_code: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct FulfillmentRow {
    order_id: Uuid,
    id: Uuid,
    provider_account_id: Uuid,
    shipping_provider: String,
    provider_reference_id: Option<String>,
    status: String,
    tracking_number: Option<String>,
    tracking_url: Option<String>,
    shipped_at: Option<OffsetDateTime>,
    delivered_at: Option<OffsetDateTime>,
    cancelled_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct OrderLineRow {
    order_id: Uuid,
    product_id: Uuid,
    product_variant_id: Uuid,
    product_title: String,
    variant_title: String,
    sku: Option<String>,
    track_inventory: bool,
    quantity: i32,
    unit_price_amount_minor: i64,
    subtotal_amount_minor: i64,
}

struct AddressFields {
    full_name: Option<String>,
    address_line1: Option<String>,
    address_line2: Option<String>,
    locality: Option<String>,
    administrative_area: Option<String>,
    postal_code: Option<String>,
    country_code: Option<String>,
}

pub(crate) async fn load(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    channel_id: Option<SalesChannelId>,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, OrderHeaderRow>(
        "SELECT order_row.id, order_row.order_number, order_row.shopper_id, cart.price_list_id, order_row.currency::text AS currency, \
                order_row.status::text AS status, order_row.payment_status::text AS payment_status, \
                order_row.fulfillment_status::text AS fulfillment_status, order_row.subtotal_amount_minor, \
                order_row.discount_amount_minor, order_row.tax_amount_minor, \
                order_row.shipping_amount_minor, order_row.total_amount_minor, order_row.amounts_finalized_at, \
                order_row.refunded_amount_minor, \
                payment_account.provider::text AS payment_provider, order_row.payment_provider_reference_id, order_row.payment_failure_code, \
                order_row.contact_email::text AS contact_email, order_row.contact_phone, \
                order_row.billing_full_name, order_row.billing_address_line1, \
                order_row.billing_address_line2, order_row.billing_locality, \
                order_row.billing_administrative_area, order_row.billing_postal_code, \
                order_row.billing_country_code::text AS billing_country_code, order_row.shipping_full_name, \
                order_row.shipping_address_line1, order_row.shipping_address_line2, \
                order_row.shipping_locality, order_row.shipping_administrative_area, \
                order_row.shipping_postal_code, order_row.shipping_country_code::text AS shipping_country_code, \
                order_row.created_at, order_row.updated_at \
         FROM chaos_commerce.orders AS order_row \
         INNER JOIN chaos_commerce.carts AS cart \
           ON cart.store_id = order_row.store_id AND cart.id = order_row.cart_id \
         INNER JOIN chaos_integration.provider_accounts AS payment_account \
           ON payment_account.id = order_row.payment_provider_account_id \
          AND payment_account.store_id = order_row.store_id \
          AND payment_account.capability = 'payment' \
         WHERE order_row.store_id = $1 \
           AND ($2::uuid IS NULL OR order_row.channel_id = $2) \
           AND order_row.id = $3",
    )
    .bind(store_id.as_uuid())
    .bind(channel_id.map(SalesChannelId::as_uuid))
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };

    let lines = sqlx::query_as::<_, OrderLineRow>(
        "SELECT order_id, product_id, product_variant_id, product_title, variant_title, sku, \
                track_inventory, quantity, unit_price_amount_minor, \
                subtotal_amount_minor FROM chaos_commerce.order_lines \
         WHERE store_id = $1 AND order_id = $2 ORDER BY position",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let refunds = sqlx::query_as::<_, RefundRow>(
        "SELECT order_id, id, status::text AS status, amount_minor, \
                payment_provider_reference_id AS provider_reference_id, \
                failure_code, created_at, updated_at \
         FROM chaos_commerce.order_refunds WHERE store_id = $1 AND order_id = $2 \
         ORDER BY created_at, id",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let fulfillments = sqlx::query_as::<_, FulfillmentRow>(
        "SELECT fulfillment.order_id, fulfillment.id, fulfillment.provider_account_id, \
                shipping_account.provider::text AS shipping_provider, \
                provider_reference_id, status::text AS status, tracking_number, \
                tracking_url, shipped_at, delivered_at, cancelled_at, fulfillment.created_at, fulfillment.updated_at \
         FROM chaos_commerce.order_fulfillments AS fulfillment \
         INNER JOIN chaos_integration.provider_accounts AS shipping_account \
           ON shipping_account.store_id = fulfillment.store_id \
          AND shipping_account.id = fulfillment.provider_account_id \
          AND shipping_account.capability = 'shipping' \
         WHERE fulfillment.store_id = $1 AND fulfillment.order_id = $2 \
         ORDER BY fulfillment.created_at, fulfillment.id",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(Some(order_detail(row, lines, refunds, fulfillments)?))
}

pub(crate) async fn load_many(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    channel_id: Option<SalesChannelId>,
    order_ids: &[Uuid],
) -> Result<HashMap<Uuid, OrderDetail>, ApplicationError> {
    if order_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query_as::<_, OrderHeaderRow>(
        "SELECT order_row.id, order_row.order_number, order_row.shopper_id, cart.price_list_id, order_row.currency::text AS currency, \
                order_row.status::text AS status, order_row.payment_status::text AS payment_status, \
                order_row.fulfillment_status::text AS fulfillment_status, order_row.subtotal_amount_minor, \
                order_row.discount_amount_minor, order_row.tax_amount_minor, \
                order_row.shipping_amount_minor, order_row.total_amount_minor, order_row.amounts_finalized_at, \
                order_row.refunded_amount_minor, \
                payment_account.provider::text AS payment_provider, order_row.payment_provider_reference_id, order_row.payment_failure_code, \
                order_row.contact_email::text AS contact_email, order_row.contact_phone, \
                order_row.billing_full_name, order_row.billing_address_line1, \
                order_row.billing_address_line2, order_row.billing_locality, \
                order_row.billing_administrative_area, order_row.billing_postal_code, \
                order_row.billing_country_code::text AS billing_country_code, order_row.shipping_full_name, \
                order_row.shipping_address_line1, order_row.shipping_address_line2, \
                order_row.shipping_locality, order_row.shipping_administrative_area, \
                order_row.shipping_postal_code, order_row.shipping_country_code::text AS shipping_country_code, \
                order_row.created_at, order_row.updated_at \
         FROM chaos_commerce.orders AS order_row \
         INNER JOIN chaos_commerce.carts AS cart \
           ON cart.store_id = order_row.store_id AND cart.id = order_row.cart_id \
         INNER JOIN chaos_integration.provider_accounts AS payment_account \
           ON payment_account.id = order_row.payment_provider_account_id \
          AND payment_account.store_id = order_row.store_id \
          AND payment_account.capability = 'payment' \
         WHERE order_row.store_id = $1 \
           AND ($2::uuid IS NULL OR order_row.channel_id = $2) \
           AND order_row.id = ANY($3::uuid[])",
    )
    .bind(store_id.as_uuid())
    .bind(channel_id.map(SalesChannelId::as_uuid))
    .bind(order_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    if rows.is_empty() {
        return Ok(HashMap::new());
    }

    let lines = sqlx::query_as::<_, OrderLineRow>(
        "SELECT order_id, product_id, product_variant_id, product_title, variant_title, sku, \
                track_inventory, quantity, unit_price_amount_minor, subtotal_amount_minor \
         FROM chaos_commerce.order_lines \
         WHERE store_id = $1 AND order_id = ANY($2::uuid[]) \
         ORDER BY order_id, position",
    )
    .bind(store_id.as_uuid())
    .bind(order_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let refunds = sqlx::query_as::<_, RefundRow>(
        "SELECT order_id, id, status::text AS status, amount_minor, \
                payment_provider_reference_id AS provider_reference_id, \
                failure_code, created_at, updated_at \
         FROM chaos_commerce.order_refunds WHERE store_id = $1 AND order_id = ANY($2::uuid[]) \
         ORDER BY order_id, created_at, id",
    )
    .bind(store_id.as_uuid())
    .bind(order_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let fulfillments = sqlx::query_as::<_, FulfillmentRow>(
        "SELECT fulfillment.order_id, fulfillment.id, fulfillment.provider_account_id, \
                shipping_account.provider::text AS shipping_provider, \
                provider_reference_id, status::text AS status, tracking_number, \
                tracking_url, shipped_at, delivered_at, cancelled_at, fulfillment.created_at, fulfillment.updated_at \
         FROM chaos_commerce.order_fulfillments AS fulfillment \
         INNER JOIN chaos_integration.provider_accounts AS shipping_account \
           ON shipping_account.store_id = fulfillment.store_id \
          AND shipping_account.id = fulfillment.provider_account_id \
          AND shipping_account.capability = 'shipping' \
         WHERE fulfillment.store_id = $1 AND fulfillment.order_id = ANY($2::uuid[]) \
         ORDER BY fulfillment.order_id, fulfillment.created_at, fulfillment.id",
    )
    .bind(store_id.as_uuid())
    .bind(order_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;

    let mut lines_by_order: HashMap<Uuid, Vec<OrderLineRow>> = HashMap::new();
    for row in lines {
        lines_by_order.entry(row.order_id).or_default().push(row);
    }
    let mut refunds_by_order: HashMap<Uuid, Vec<RefundRow>> = HashMap::new();
    for row in refunds {
        refunds_by_order.entry(row.order_id).or_default().push(row);
    }
    let mut fulfillments_by_order: HashMap<Uuid, Vec<FulfillmentRow>> = HashMap::new();
    for row in fulfillments {
        fulfillments_by_order
            .entry(row.order_id)
            .or_default()
            .push(row);
    }

    rows.into_iter()
        .map(|row| {
            let order_id = row.id;
            let detail = order_detail(
                row,
                lines_by_order.remove(&order_id).unwrap_or_default(),
                refunds_by_order.remove(&order_id).unwrap_or_default(),
                fulfillments_by_order.remove(&order_id).unwrap_or_default(),
            )?;
            Ok((order_id, detail))
        })
        .collect()
}

fn order_detail(
    row: OrderHeaderRow,
    lines: Vec<OrderLineRow>,
    refunds: Vec<RefundRow>,
    fulfillments: Vec<FulfillmentRow>,
) -> Result<OrderDetail, ApplicationError> {
    Ok(OrderDetail {
        id: OrderId::from_uuid(row.id),
        order_number: OrderNumber::parse(&row.order_number)?,
        shopper_id: ShopperId::from_uuid(row.shopper_id),
        price_list_id: PriceListId::from_uuid(row.price_list_id),
        currency: CurrencyCode::parse(&row.currency)?,
        status: OrderStatus::parse(&row.status).ok_or_else(corrupt_state)?,
        payment_status: OrderPaymentStatus::parse(&row.payment_status).ok_or_else(corrupt_state)?,
        fulfillment_status: FulfillmentStatus::parse(&row.fulfillment_status)
            .ok_or_else(corrupt_state)?,
        payment_provider: row
            .payment_provider
            .as_deref()
            .map(|value| PaymentProvider::parse(value).ok_or_else(corrupt_state))
            .transpose()?,
        payment_provider_reference_id: row.payment_provider_reference_id.clone(),
        identity: order_identity(&row)?,
        subtotal_amount_minor: row.subtotal_amount_minor,
        discount_amount_minor: row.discount_amount_minor,
        tax_amount_minor: row.tax_amount_minor,
        shipping_amount_minor: row.shipping_amount_minor,
        total_amount_minor: row.total_amount_minor,
        amounts_finalized_at: row.amounts_finalized_at,
        refunded_amount_minor: row.refunded_amount_minor,
        lines: lines
            .into_iter()
            .map(order_line_item)
            .collect::<Result<_, _>>()?,
        payment_attempt: payment_attempt_item(&row)?,
        refunds: refunds
            .into_iter()
            .map(refund_item)
            .collect::<Result<_, _>>()?,
        fulfillments: fulfillments
            .into_iter()
            .map(fulfillment_item)
            .collect::<Result<_, _>>()?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn order_identity(row: &OrderHeaderRow) -> Result<OrderIdentity, ApplicationError> {
    Ok(OrderIdentity::new(
        OrderContact::new(
            normalize_optional_text(row.contact_email.clone()),
            normalize_optional_text(row.contact_phone.clone()),
        )?,
        optional_address(AddressFields {
            full_name: row.billing_full_name.clone(),
            address_line1: row.billing_address_line1.clone(),
            address_line2: row.billing_address_line2.clone(),
            locality: row.billing_locality.clone(),
            administrative_area: row.billing_administrative_area.clone(),
            postal_code: row.billing_postal_code.clone(),
            country_code: row.billing_country_code.clone(),
        })?,
        optional_address(AddressFields {
            full_name: row.shipping_full_name.clone(),
            address_line1: row.shipping_address_line1.clone(),
            address_line2: row.shipping_address_line2.clone(),
            locality: row.shipping_locality.clone(),
            administrative_area: row.shipping_administrative_area.clone(),
            postal_code: row.shipping_postal_code.clone(),
            country_code: row.shipping_country_code.clone(),
        })?,
    ))
}

fn optional_address(fields: AddressFields) -> Result<Option<PostalAddress>, ApplicationError> {
    let full_name = normalize_optional_text(fields.full_name);
    let address_line1 = normalize_optional_text(fields.address_line1);
    let address_line2 = normalize_optional_text(fields.address_line2);
    let locality = normalize_optional_text(fields.locality);
    let administrative_area = normalize_optional_text(fields.administrative_area);
    let postal_code = normalize_optional_text(fields.postal_code);
    let country_code = normalize_optional_text(fields.country_code);
    let any = full_name.is_some()
        || address_line1.is_some()
        || address_line2.is_some()
        || locality.is_some()
        || administrative_area.is_some()
        || postal_code.is_some()
        || country_code.is_some();
    match (full_name, address_line1, locality, country_code) {
        (None, None, None, None) if !any => Ok(None),
        (Some(full_name), Some(address_line1), Some(locality), Some(country_code)) => {
            Ok(Some(PostalAddress::new(
                full_name,
                address_line1,
                address_line2,
                locality,
                administrative_area,
                postal_code,
                country_code,
            )?))
        }
        _ => Err(corrupt_state()),
    }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn order_line_item(row: OrderLineRow) -> Result<OrderLineItem, ApplicationError> {
    Ok(OrderLineItem {
        product_id: ProductId::from_uuid(row.product_id),
        product_variant_id: ProductVariantId::from_uuid(row.product_variant_id),
        product_title: row.product_title,
        variant_title: row.variant_title,
        sku: row.sku,
        track_inventory: row.track_inventory,
        quantity: u32::try_from(row.quantity)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        unit_price_amount_minor: row.unit_price_amount_minor,
        subtotal_amount_minor: row.subtotal_amount_minor,
    })
}

/// The Order's payment attempt exists once checkout has actually produced a
/// Provider reference or a failure — a still-`pending` Order with neither is
/// one whose checkout was never started.
fn payment_attempt_item(
    row: &OrderHeaderRow,
) -> Result<Option<OrderPaymentAttemptItem>, ApplicationError> {
    if row.payment_provider_reference_id.is_none() && row.payment_failure_code.is_none() {
        return Ok(None);
    }
    let status = match row.payment_status.as_str() {
        "paid" | "partially_refunded" | "refunded" => PaymentAttemptStatus::Captured,
        "failed" => PaymentAttemptStatus::Failed,
        "expired" => PaymentAttemptStatus::Expired,
        _ => PaymentAttemptStatus::Pending,
    };
    Ok(Some(OrderPaymentAttemptItem {
        status,
        amount_minor: row.total_amount_minor,
        provider_reference_id: row.payment_provider_reference_id.clone(),
        failure_code: row.payment_failure_code.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

fn refund_item(row: RefundRow) -> Result<OrderRefundItem, ApplicationError> {
    Ok(OrderRefundItem {
        id: RefundId::from_uuid(row.id),
        status: RefundStatus::parse(&row.status).ok_or_else(corrupt_state)?,
        amount_minor: row.amount_minor,
        provider_reference_id: row.provider_reference_id,
        failure_code: row.failure_code,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn fulfillment_item(row: FulfillmentRow) -> Result<OrderFulfillmentItem, ApplicationError> {
    Ok(OrderFulfillmentItem {
        id: FulfillmentId::from_uuid(row.id),
        provider_account_id: FulfillmentProviderAccountId::from_uuid(row.provider_account_id),
        shipping_provider: FulfillmentProvider::parse(&row.shipping_provider)
            .ok_or_else(corrupt_state)?,
        provider_reference_id: row.provider_reference_id,
        status: FulfillmentStatus::parse(&row.status).ok_or_else(corrupt_state)?,
        tracking_number: row.tracking_number,
        tracking_url: row.tracking_url,
        shipped_at: row.shipped_at,
        delivered_at: row.delivered_at,
        cancelled_at: row.cancelled_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains an unknown order state"))
}

#[cfg(test)]
mod tests {
    use super::normalize_optional_text;

    #[test]
    fn normalizes_blank_database_optional_text() {
        assert_eq!(normalize_optional_text(Some("  ".into())), None);
        assert_eq!(
            normalize_optional_text(Some(" Suite 100 ".into())),
            Some("Suite 100".into())
        );
    }
}
