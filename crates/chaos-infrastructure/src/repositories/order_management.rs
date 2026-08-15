use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        IdempotencyRequest, OrderDetail, OrderLineItem, OrderManagementRepository,
        OrderTransitionItem,
    },
};
use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
    fulfillment::{ShippingSelection, ShippingServiceId},
    inventory::InventoryReservationId,
    merchant::StoreId,
    pricing::{Money, PriceListId},
    sales::{
        CheckoutContact, CheckoutId, CheckoutIdentity, Order, OrderId, OrderStatus, PostalAddress,
        ShopperId,
    },
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    idempotency::{self, IdempotencyScope},
    inventory::{ReservationClosure, close_reservation},
};

const CONFIRM_OPERATION: &str = "orders.confirm.v1";
const CANCEL_OPERATION: &str = "orders.cancel.v1";

type HeaderRow = (
    Uuid,
    Uuid,
    Uuid,
    Option<Uuid>,
    Uuid,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    OffsetDateTime,
    OffsetDateTime,
);
type LineRow = (
    Uuid,
    Uuid,
    String,
    String,
    Option<String>,
    bool,
    bool,
    i32,
    i64,
    i64,
    i64,
    i64,
    i64,
    bool,
);
type AddressRow = (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
);

#[derive(Clone)]
pub struct PostgresOrderManagementRepository {
    pool: PgPool,
}

impl PostgresOrderManagementRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: MerchantActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        for (key, value) in [
            ("app.user_id", actor.user_id().as_uuid()),
            (
                "app.merchant_account_id",
                actor.merchant_account_id().as_uuid(),
            ),
        ] {
            sqlx::query("SELECT set_config($1, $2, true)")
                .bind(key)
                .bind(value.to_string())
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
        }
        Ok(transaction)
    }
}

#[async_trait]
impl OrderManagementRepository for PostgresOrderManagementRepository {
    async fn get_order(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let detail = load_order(
            &mut transaction,
            actor.merchant_account_id().as_uuid(),
            store_id,
            order_id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn transition_order(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        order_id: OrderId,
        target_status: OrderStatus,
        now: OffsetDateTime,
        request: &IdempotencyRequest,
    ) -> Result<OrderDetail, ApplicationError> {
        let operation = match target_status {
            OrderStatus::Confirmed => CONFIRM_OPERATION,
            OrderStatus::Cancelled => CANCEL_OPERATION,
            OrderStatus::Pending => return Err(invalid_target()),
        };
        let account_id = actor.merchant_account_id().as_uuid();
        let mut transaction = self.begin(actor).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::MerchantAccount(account_id),
            operation,
            request,
        )
        .await?
        {
            let replay_id = snapshot
                .get("id")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(OrderId::from_uuid)
                .ok_or_else(corrupt_snapshot)?;
            return load_order(&mut transaction, account_id, store_id, replay_id)
                .await?
                .ok_or_else(|| order_not_found(replay_id));
        }
        let row = sqlx::query_as::<_, (Uuid, String, Option<Uuid>)>(
            "SELECT checkout_id, status::text, inventory_reservation_id FROM sales.orders \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3 FOR UPDATE",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        let current_status = OrderStatus::parse(&row.1).ok_or_else(corrupt_state)?;
        let mut order = Order::rehydrate(order_id, CheckoutId::from_uuid(row.0), current_status);
        let transition = match target_status {
            OrderStatus::Confirmed => order.confirm(now)?,
            OrderStatus::Cancelled => order.cancel(now)?,
            OrderStatus::Pending => return Err(invalid_target()),
        };
        if let Some(reservation_id) = row.2.map(InventoryReservationId::from_uuid) {
            close_reservation(
                &mut transaction,
                account_id,
                store_id,
                reservation_id,
                if target_status == OrderStatus::Confirmed {
                    ReservationClosure::Consumed
                } else {
                    ReservationClosure::Released
                },
                now,
            )
            .await?;
        }
        sqlx::query(
            "UPDATE sales.orders SET status = $4::sales.order_status, updated_at = $5 \
             WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
        )
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(target_status.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO sales.order_transitions \
             (id, merchant_account_id, store_id, order_id, from_status, to_status, kind, \
              actor_user_id, occurred_at) \
             VALUES ($1, $2, $3, $4, $5::sales.order_status, $6::sales.order_status, \
                     $7::sales.order_transition_kind, $8, $9)",
        )
        .bind(Uuid::now_v7())
        .bind(account_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(transition.from_status.map(OrderStatus::as_str))
        .bind(transition.to_status.as_str())
        .bind(transition.kind.as_str())
        .bind(actor.user_id().as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        idempotency::complete(
            &mut transaction,
            &IdempotencyScope::MerchantAccount(account_id),
            operation,
            request,
            200,
            json!({"id": order_id.as_uuid()}),
        )
        .await?;
        let detail = load_order(&mut transaction, account_id, store_id, order_id)
            .await?
            .ok_or_else(|| order_not_found(order_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
}

async fn load_order(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, HeaderRow>(
        "SELECT id, shopper_id, checkout_id, inventory_reservation_id, price_list_id, currency::text, \
                status::text, subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                shipping_amount_minor, total_amount_minor, created_at, updated_at FROM sales.orders \
         WHERE merchant_account_id = $1 AND store_id = $2 AND id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let identity = load_order_identity(transaction, account_id, store_id, order_id).await?;
    let shipping = load_order_shipping(transaction, account_id, store_id, order_id).await?;
    let lines = sqlx::query_as::<_, LineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor, \
                subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                total_amount_minor, tax_inclusive FROM sales.order_lines \
         WHERE merchant_account_id = $1 AND store_id = $2 AND order_id = $3 ORDER BY position",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
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
         FROM sales.order_transitions WHERE merchant_account_id = $1 AND store_id = $2 \
           AND order_id = $3 ORDER BY occurred_at, id",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(Some(OrderDetail {
        id: OrderId::from_uuid(row.0),
        shopper_id: ShopperId::from_uuid(row.1),
        checkout_id: CheckoutId::from_uuid(row.2),
        inventory_reservation_id: row.3.map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(row.4),
        currency: CurrencyCode::parse(&row.5)?,
        status: OrderStatus::parse(&row.6).ok_or_else(corrupt_state)?,
        identity,
        subtotal_amount_minor: row.7,
        discount_amount_minor: row.8,
        tax_amount_minor: row.9,
        shipping,
        shipping_amount_minor: row.10,
        total_amount_minor: row.11,
        lines: lines
            .into_iter()
            .map(|line| {
                Ok(OrderLineItem {
                    product_id: ProductId::from_uuid(line.0),
                    product_variant_id: ProductVariantId::from_uuid(line.1),
                    product_title: line.2,
                    variant_title: line.3,
                    sku: line.4,
                    requires_shipping: line.5,
                    track_inventory: line.6,
                    quantity: u32::try_from(line.7)
                        .map_err(|error| ApplicationError::Unexpected(error.into()))?,
                    unit_price_amount_minor: line.8,
                    subtotal_amount_minor: line.9,
                    discount_amount_minor: line.10,
                    tax_amount_minor: line.11,
                    total_amount_minor: line.12,
                    tax_inclusive: line.13,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        transitions: transitions
            .into_iter()
            .map(|item| {
                Ok(OrderTransitionItem {
                    id: item.0,
                    from_status: item
                        .1
                        .as_deref()
                        .map(|status| OrderStatus::parse(status).ok_or_else(corrupt_state))
                        .transpose()?,
                    to_status: OrderStatus::parse(&item.2).ok_or_else(corrupt_state)?,
                    kind: item.3,
                    actor_user_id: item.4,
                    occurred_at: item.5,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?,
        created_at: row.12,
        updated_at: row.13,
    }))
}

async fn load_order_shipping(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<Option<ShippingSelection>, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, i64, String, i16, i16)>(
        "SELECT shipping_service_id, service_code, service_name, amount_minor, currency::text, \
                estimated_min_days, estimated_max_days \
         FROM sales.order_shipping_selections \
         WHERE merchant_account_id = $1 AND store_id = $2 AND order_id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        ShippingSelection::rehydrate(
            ShippingServiceId::from_uuid(row.0),
            row.1,
            row.2,
            Money::new(row.3, CurrencyCode::parse(&row.4)?),
            u16::try_from(row.5).map_err(|error| ApplicationError::Unexpected(error.into()))?,
            u16::try_from(row.6).map_err(|error| ApplicationError::Unexpected(error.into()))?,
        )
        .map_err(ApplicationError::from)
    })
    .transpose()
}

async fn load_order_identity(
    transaction: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<CheckoutIdentity, ApplicationError> {
    let contact = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT email::text, phone FROM sales.order_contacts \
         WHERE merchant_account_id = $1 AND store_id = $2 AND order_id = $3",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_state)?;
    let addresses = sqlx::query_as::<_, AddressRow>(
        "SELECT kind::text, full_name, company, address_line1, address_line2, locality, \
                administrative_area, postal_code, country_code::text \
         FROM sales.order_addresses \
         WHERE merchant_account_id = $1 AND store_id = $2 AND order_id = $3 ORDER BY kind",
    )
    .bind(account_id)
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let contact = CheckoutContact::new(contact.0, contact.1)?;
    let mut billing = None;
    let mut shipping = None;
    for row in addresses {
        let kind = row.0.clone();
        let address = PostalAddress::new(row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8)?;
        match kind.as_str() {
            "billing" if billing.is_none() => billing = Some(address),
            "shipping" if shipping.is_none() => shipping = Some(address),
            _ => return Err(corrupt_state()),
        }
    }
    Ok(CheckoutIdentity::new(
        contact,
        billing.ok_or_else(corrupt_state)?,
        shipping,
    ))
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn invalid_target() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "status",
            reason: "must be confirmed or cancelled".into(),
        }],
    }
}

fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains an unknown Order state"))
}

fn corrupt_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("invalid Order idempotency snapshot"))
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            ApplicationError::Unavailable {
                service: "postgresql",
                source: error.into(),
            }
        }
        _ => ApplicationError::Unexpected(error.into()),
    }
}
