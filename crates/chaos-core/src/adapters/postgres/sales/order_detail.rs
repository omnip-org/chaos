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
    fulfillment::{FulfillmentId, FulfillmentStatus, ShippingProviderAccountId},
    integration::{PaymentProvider, ShippingProvider},
    payments::{PaymentAttemptStatus, RefundId, RefundStatus},
    pricing::PriceListId,
    sales::{
        OrderContact, OrderId, OrderIdentity, OrderNumber, OrderPaymentStatus, OrderShippingStatus,
        OrderStatus, PostalAddress, ShopperId,
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
    shipping_status: String,
    subtotal_amount_minor: i64,
    discount_amount_minor: i64,
    tax_amount_minor: i64,
    shipping_amount_minor: i64,
    total_amount_minor: i64,
    refunded_amount_minor: i64,
    payment_provider: Option<String>,
    payment_provider_reference_id: Option<String>,
    payment_failure_code: Option<String>,
    shipping_provider: Option<String>,
    shipping_provider_reference_id: Option<String>,
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
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct RefundRow {
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
    id: Uuid,
    shipping_provider_account_id: Uuid,
    status: String,
    tracking_number: Option<String>,
    tracking_url: Option<String>,
    shipped_at: Option<OffsetDateTime>,
    delivered_at: Option<OffsetDateTime>,
    cancelled_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

type OrderLineRow = (
    Uuid,
    Uuid,
    String,
    String,
    Option<String>,
    bool,
    i32,
    i64,
    i64,
);

type BatchOrderLineRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    String,
    Option<String>,
    bool,
    i32,
    i64,
    i64,
);

#[derive(sqlx::FromRow)]
struct BatchRefundRow {
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
struct BatchFulfillmentRow {
    order_id: Uuid,
    id: Uuid,
    shipping_provider_account_id: Uuid,
    status: String,
    tracking_number: Option<String>,
    tracking_url: Option<String>,
    shipped_at: Option<OffsetDateTime>,
    delivered_at: Option<OffsetDateTime>,
    cancelled_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

pub(crate) async fn load(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    channel_id: Option<SalesChannelId>,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, OrderHeaderRow>(
        "SELECT order_row.id, order_row.order_number, order_row.shopper_id, order_row.price_list_id, order_row.currency::text AS currency, \
                order_row.status::text AS status, order_row.payment_status::text AS payment_status, \
                order_row.shipping_status::text AS shipping_status, order_row.subtotal_amount_minor, \
                order_row.discount_amount_minor, order_row.tax_amount_minor, \
                order_row.shipping_amount_minor, order_row.total_amount_minor, order_row.refunded_amount_minor, \
                payment_account.provider::text AS payment_provider, order_row.payment_provider_reference_id, order_row.payment_failure_code, \
                shipping_account.provider::text AS shipping_provider, order_row.shipping_provider_reference_id, \
                order_row.contact_email::text AS contact_email, order_row.contact_phone, \
                order_row.billing_full_name, order_row.billing_company, order_row.billing_address_line1, \
                order_row.billing_address_line2, order_row.billing_locality, \
                order_row.billing_administrative_area, order_row.billing_postal_code, \
                order_row.billing_country_code::text AS billing_country_code, order_row.shipping_full_name, \
                order_row.shipping_company, order_row.shipping_address_line1, order_row.shipping_address_line2, \
                order_row.shipping_locality, order_row.shipping_administrative_area, \
                order_row.shipping_postal_code, order_row.shipping_country_code::text AS shipping_country_code, \
                order_row.created_at, order_row.updated_at \
         FROM commerce.orders AS order_row \
         INNER JOIN integration.provider_accounts AS payment_account \
           ON payment_account.id = order_row.payment_provider_account_id \
          AND payment_account.store_id = order_row.store_id \
          AND payment_account.capability = 'payment' \
         LEFT JOIN integration.provider_accounts AS shipping_account \
           ON shipping_account.id = order_row.shipping_provider_account_id \
          AND shipping_account.store_id = order_row.store_id \
          AND shipping_account.capability = 'shipping' \
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

    let identity = order_identity(&row)?;
    let lines = sqlx::query_as::<_, OrderLineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                track_inventory, quantity, unit_price_amount_minor, \
                subtotal_amount_minor FROM commerce.order_lines \
         WHERE store_id = $1 AND order_id = $2 ORDER BY position",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let refunds = sqlx::query_as::<_, RefundRow>(
        "SELECT id, status::text, amount_minor, \
                payment_provider_reference_id AS provider_reference_id, \
                failure_code, created_at, updated_at \
         FROM commerce.order_refunds WHERE store_id = $1 AND order_id = $2 \
         ORDER BY created_at, id",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let fulfillments = sqlx::query_as::<_, FulfillmentRow>(
        "SELECT id, shipping_provider_account_id, status::text, tracking_number, \
                tracking_url, shipped_at, delivered_at, cancelled_at, created_at, updated_at \
         FROM commerce.order_fulfillments WHERE store_id = $1 AND order_id = $2 \
         ORDER BY created_at, id",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let order_number = OrderNumber::parse(&row.order_number)?;

    Ok(Some(OrderDetail {
        id: OrderId::from_uuid(row.id),
        order_number,
        shopper_id: ShopperId::from_uuid(row.shopper_id),
        price_list_id: PriceListId::from_uuid(row.price_list_id),
        currency: CurrencyCode::parse(&row.currency)?,
        status: OrderStatus::parse(&row.status).ok_or_else(corrupt_state)?,
        payment_status: OrderPaymentStatus::parse(&row.payment_status).ok_or_else(corrupt_state)?,
        shipping_status: OrderShippingStatus::parse(&row.shipping_status)
            .ok_or_else(corrupt_state)?,
        payment_provider: row
            .payment_provider
            .as_deref()
            .map(|value| PaymentProvider::parse(value).ok_or_else(corrupt_state))
            .transpose()?,
        payment_provider_reference_id: row.payment_provider_reference_id.clone(),
        shipping_provider: row
            .shipping_provider
            .as_deref()
            .map(|value| ShippingProvider::parse(value).ok_or_else(corrupt_state))
            .transpose()?,
        shipping_provider_reference_id: row.shipping_provider_reference_id.clone(),
        identity,
        subtotal_amount_minor: row.subtotal_amount_minor,
        discount_amount_minor: row.discount_amount_minor,
        tax_amount_minor: row.tax_amount_minor,
        shipping_amount_minor: row.shipping_amount_minor,
        total_amount_minor: row.total_amount_minor,
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
    }))
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
        "SELECT order_row.id, order_row.order_number, order_row.shopper_id, order_row.price_list_id, order_row.currency::text AS currency, \
                order_row.status::text AS status, order_row.payment_status::text AS payment_status, \
                order_row.shipping_status::text AS shipping_status, order_row.subtotal_amount_minor, \
                order_row.discount_amount_minor, order_row.tax_amount_minor, \
                order_row.shipping_amount_minor, order_row.total_amount_minor, order_row.refunded_amount_minor, \
                payment_account.provider::text AS payment_provider, order_row.payment_provider_reference_id, order_row.payment_failure_code, \
                shipping_account.provider::text AS shipping_provider, order_row.shipping_provider_reference_id, \
                order_row.contact_email::text AS contact_email, order_row.contact_phone, \
                order_row.billing_full_name, order_row.billing_company, order_row.billing_address_line1, \
                order_row.billing_address_line2, order_row.billing_locality, \
                order_row.billing_administrative_area, order_row.billing_postal_code, \
                order_row.billing_country_code::text AS billing_country_code, order_row.shipping_full_name, \
                order_row.shipping_company, order_row.shipping_address_line1, order_row.shipping_address_line2, \
                order_row.shipping_locality, order_row.shipping_administrative_area, \
                order_row.shipping_postal_code, order_row.shipping_country_code::text AS shipping_country_code, \
                order_row.created_at, order_row.updated_at \
         FROM commerce.orders AS order_row \
         INNER JOIN integration.provider_accounts AS payment_account \
           ON payment_account.id = order_row.payment_provider_account_id \
          AND payment_account.store_id = order_row.store_id \
          AND payment_account.capability = 'payment' \
         LEFT JOIN integration.provider_accounts AS shipping_account \
           ON shipping_account.id = order_row.shipping_provider_account_id \
          AND shipping_account.store_id = order_row.store_id \
          AND shipping_account.capability = 'shipping' \
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

    let lines = sqlx::query_as::<_, BatchOrderLineRow>(
        "SELECT order_id, product_id, product_variant_id, product_title, variant_title, sku, \
                track_inventory, quantity, unit_price_amount_minor, subtotal_amount_minor \
         FROM commerce.order_lines \
         WHERE store_id = $1 AND order_id = ANY($2::uuid[]) \
         ORDER BY order_id, position",
    )
    .bind(store_id.as_uuid())
    .bind(order_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let refunds = sqlx::query_as::<_, BatchRefundRow>(
        "SELECT order_id, id, status::text, amount_minor, \
                payment_provider_reference_id AS provider_reference_id, \
                failure_code, created_at, updated_at \
         FROM commerce.order_refunds WHERE store_id = $1 AND order_id = ANY($2::uuid[]) \
         ORDER BY order_id, created_at, id",
    )
    .bind(store_id.as_uuid())
    .bind(order_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let fulfillments = sqlx::query_as::<_, BatchFulfillmentRow>(
        "SELECT order_id, id, shipping_provider_account_id, status::text, tracking_number, \
                tracking_url, shipped_at, delivered_at, cancelled_at, created_at, updated_at \
         FROM commerce.order_fulfillments WHERE store_id = $1 AND order_id = ANY($2::uuid[]) \
         ORDER BY order_id, created_at, id",
    )
    .bind(store_id.as_uuid())
    .bind(order_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;

    let mut lines_by_order: HashMap<Uuid, Vec<BatchOrderLineRow>> = HashMap::new();
    for row in lines {
        lines_by_order.entry(row.0).or_default().push(row);
    }
    let mut refunds_by_order: HashMap<Uuid, Vec<BatchRefundRow>> = HashMap::new();
    for row in refunds {
        refunds_by_order.entry(row.order_id).or_default().push(row);
    }
    let mut fulfillments_by_order: HashMap<Uuid, Vec<BatchFulfillmentRow>> = HashMap::new();
    for row in fulfillments {
        fulfillments_by_order
            .entry(row.order_id)
            .or_default()
            .push(row);
    }

    rows.into_iter()
        .map(|row| {
            let order_id = row.id;
            let detail = OrderDetail {
                id: OrderId::from_uuid(order_id),
                order_number: OrderNumber::parse(&row.order_number)?,
                shopper_id: ShopperId::from_uuid(row.shopper_id),
                price_list_id: PriceListId::from_uuid(row.price_list_id),
                currency: CurrencyCode::parse(&row.currency)?,
                status: OrderStatus::parse(&row.status).ok_or_else(corrupt_state)?,
                payment_status: OrderPaymentStatus::parse(&row.payment_status)
                    .ok_or_else(corrupt_state)?,
                shipping_status: OrderShippingStatus::parse(&row.shipping_status)
                    .ok_or_else(corrupt_state)?,
                payment_provider: row
                    .payment_provider
                    .as_deref()
                    .map(|value| PaymentProvider::parse(value).ok_or_else(corrupt_state))
                    .transpose()?,
                payment_provider_reference_id: row.payment_provider_reference_id.clone(),
                shipping_provider: row
                    .shipping_provider
                    .as_deref()
                    .map(|value| ShippingProvider::parse(value).ok_or_else(corrupt_state))
                    .transpose()?,
                shipping_provider_reference_id: row.shipping_provider_reference_id.clone(),
                identity: order_identity(&row)?,
                subtotal_amount_minor: row.subtotal_amount_minor,
                discount_amount_minor: row.discount_amount_minor,
                tax_amount_minor: row.tax_amount_minor,
                shipping_amount_minor: row.shipping_amount_minor,
                total_amount_minor: row.total_amount_minor,
                refunded_amount_minor: row.refunded_amount_minor,
                lines: lines_by_order
                    .remove(&order_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|line| {
                        order_line_item((
                            line.1, line.2, line.3, line.4, line.5, line.6, line.7, line.8, line.9,
                        ))
                    })
                    .collect::<Result<_, _>>()?,
                payment_attempt: payment_attempt_item(&row)?,
                refunds: refunds_by_order
                    .remove(&order_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|refund| {
                        refund_item(RefundRow {
                            id: refund.id,
                            status: refund.status,
                            amount_minor: refund.amount_minor,
                            provider_reference_id: refund.provider_reference_id,
                            failure_code: refund.failure_code,
                            created_at: refund.created_at,
                            updated_at: refund.updated_at,
                        })
                    })
                    .collect::<Result<_, _>>()?,
                fulfillments: fulfillments_by_order
                    .remove(&order_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|fulfillment| {
                        fulfillment_item(FulfillmentRow {
                            id: fulfillment.id,
                            shipping_provider_account_id: fulfillment.shipping_provider_account_id,
                            status: fulfillment.status,
                            tracking_number: fulfillment.tracking_number,
                            tracking_url: fulfillment.tracking_url,
                            shipped_at: fulfillment.shipped_at,
                            delivered_at: fulfillment.delivered_at,
                            cancelled_at: fulfillment.cancelled_at,
                            created_at: fulfillment.created_at,
                            updated_at: fulfillment.updated_at,
                        })
                    })
                    .collect::<Result<_, _>>()?,
                created_at: row.created_at,
                updated_at: row.updated_at,
            };
            Ok((order_id, detail))
        })
        .collect()
}

fn order_identity(row: &OrderHeaderRow) -> Result<OrderIdentity, ApplicationError> {
    Ok(OrderIdentity::new(
        OrderContact::new(
            normalize_optional_text(row.contact_email.clone()),
            normalize_optional_text(row.contact_phone.clone()),
        )?,
        optional_address(
            row.billing_full_name.clone(),
            row.billing_company.clone(),
            row.billing_address_line1.clone(),
            row.billing_address_line2.clone(),
            row.billing_locality.clone(),
            row.billing_administrative_area.clone(),
            row.billing_postal_code.clone(),
            row.billing_country_code.clone(),
        )?,
        optional_address(
            row.shipping_full_name.clone(),
            row.shipping_company.clone(),
            row.shipping_address_line1.clone(),
            row.shipping_address_line2.clone(),
            row.shipping_locality.clone(),
            row.shipping_administrative_area.clone(),
            row.shipping_postal_code.clone(),
            row.shipping_country_code.clone(),
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
    let full_name = normalize_optional_text(full_name);
    let company = normalize_optional_text(company);
    let address_line1 = normalize_optional_text(address_line1);
    let address_line2 = normalize_optional_text(address_line2);
    let locality = normalize_optional_text(locality);
    let administrative_area = normalize_optional_text(administrative_area);
    let postal_code = normalize_optional_text(postal_code);
    let country_code = normalize_optional_text(country_code);
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
        (Some(full_name), Some(address_line1), Some(locality), Some(country_code)) => {
            Ok(Some(PostalAddress::new(
                full_name,
                company,
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
        product_id: ProductId::from_uuid(row.0),
        product_variant_id: ProductVariantId::from_uuid(row.1),
        product_title: row.2,
        variant_title: row.3,
        sku: row.4,
        track_inventory: row.5,
        quantity: u32::try_from(row.6)
            .map_err(|error| ApplicationError::Unexpected(error.into()))?,
        unit_price_amount_minor: row.7,
        subtotal_amount_minor: row.8,
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
        shipping_provider_account_id: ShippingProviderAccountId::from_uuid(
            row.shipping_provider_account_id,
        ),
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
