use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, OrderDetail, OrderLineItem, OrderListFilter,
        OrderManagementRepository, OrderPage, OrderTransitionItem,
    },
};
use chaos_domain::{
    CurrencyCode, Locale,
    catalog::{ProductId, ProductVariantId},
    fulfillment::{ShippingSelection, ShippingServiceId},
    inventory::InventoryReservationId,
    pricing::{
        Money, PriceListId, PromotionId, PromotionSnapshot, PromotionTrigger, PromotionValue,
        TaxRuleId, TaxRuleSnapshot,
    },
    sales::{
        CheckoutContact, CheckoutId, CheckoutIdentity, CustomerId, Order, OrderDeliveryStatus,
        OrderFulfillmentStatus, OrderId, OrderNumber, OrderStatus, PostalAddress, ShopperId,
    },
    store::StoreId,
};
use rand::Rng;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

const ORDER_TRACKING_KEY_LIFETIME: time::Duration = time::Duration::days(180);

fn generate_order_tracking_key() -> (String, [u8; 32]) {
    let mut secret = [0_u8; 32];
    rand::rng().fill_bytes(&mut secret);
    let plaintext = format!("otk_{}", URL_SAFE_NO_PAD.encode(secret));
    let digest = Sha256::digest(plaintext.as_bytes()).into();
    (plaintext, digest)
}

use super::{
    idempotency::{self, IdempotencyScope},
    inventory::{ReservationClosure, close_reservation},
};

const CONFIRM_OPERATION: &str = "orders.confirm.v1";
const CANCEL_OPERATION: &str = "orders.cancel.v1";

type HeaderRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    Uuid,
    Option<Uuid>,
    Uuid,
    String,
    String,
    i64,
    i64,
    i64,
    bool,
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

    async fn begin_for_admin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(actor.store_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

#[async_trait]
impl OrderManagementRepository for PostgresOrderManagementRepository {
    async fn list_orders(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
        filter: &OrderListFilter,
    ) -> Result<OrderPage, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT o.id FROM commerce.orders o \
             JOIN commerce.order_contacts contact ON contact.store_id = o.store_id \
               AND contact.order_id = o.id \
             LEFT JOIN commerce.customer_shopper_links link ON link.store_id = o.store_id \
               AND link.shopper_id = o.shopper_id \
             WHERE o.store_id = $1 \
               AND ($2::uuid IS NULL OR o.id < $2) \
               AND ($3::text IS NULL OR o.status::text = $3) \
               AND ($4::uuid IS NULL OR o.customer_id = $4 OR link.customer_id = $4) \
               AND ($5::text IS NULL OR contact.email = lower($5)) \
               AND ($6::text IS NULL OR o.order_number = upper($6)) \
             ORDER BY o.id DESC LIMIT $7",
        )
        .bind(store_id.as_uuid())
        .bind(after)
        .bind(filter.status.map(OrderStatus::as_str))
        .bind(filter.customer_id.map(CustomerId::as_uuid))
        .bind(filter.email.as_deref())
        .bind(filter.order_number.as_deref())
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let has_more = ids.len() > usize::from(limit);
        let mut items = Vec::with_capacity(ids.len().min(usize::from(limit)));
        for id in ids.into_iter().take(usize::from(limit)) {
            items.push(
                load_order(&mut transaction, store_id, OrderId::from_uuid(id))
                    .await?
                    .ok_or_else(|| order_not_found(OrderId::from_uuid(id)))?,
            );
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(OrderPage { items, has_more })
    }

    async fn get_order(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        let detail = load_order(&mut transaction, store_id, order_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    async fn transition_order(
        &self,
        actor: AdminActor,
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
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut transaction = self.begin_for_admin(&actor).await?;
        if let Some(snapshot) = idempotency::reserve(
            &mut transaction,
            &IdempotencyScope::Store(store_id.as_uuid()),
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
            return load_order(&mut transaction, store_id, replay_id)
                .await?
                .ok_or_else(|| order_not_found(replay_id));
        }
        let row = sqlx::query_as::<_, (Uuid, String, Option<Uuid>)>(
            "SELECT checkout_id, status::text, inventory_reservation_id FROM commerce.orders \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
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
        let transition_id = Uuid::now_v7();
        sqlx::query(
            "UPDATE commerce.orders SET status = $3::commerce.order_status, updated_at = $4 \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(target_status.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO commerce.order_transitions \
             (id, store_id, order_id, from_status, to_status, kind, \
              actor_user_id, occurred_at) \
             VALUES ($1, $2, $3, $4::commerce.order_status, $5::commerce.order_status, \
                     $6::commerce.order_transition_kind, $7, $8)",
        )
        .bind(transition_id)
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(transition.from_status.map(OrderStatus::as_str))
        .bind(transition.to_status.as_str())
        .bind(transition.kind.as_str())
        .bind(audit_user_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if target_status == OrderStatus::Confirmed {
            let (tracking_key, tracking_digest) = generate_order_tracking_key();
            sqlx::query(
                "INSERT INTO commerce.order_tracking_keys \
                 (id,store_id,order_id,secret_digest,expires_at,created_at) \
                 VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(store_id,order_id) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(store_id.as_uuid())
            .bind(order_id.as_uuid())
            .bind(tracking_digest.as_slice())
            .bind(now + ORDER_TRACKING_KEY_LIFETIME)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO integration.email_deliveries \
                 (id, store_id, semantic_event_id, semantic_event_type, \
                  recipient_email, template_key, template_version, template_payload, provider) \
                 SELECT $1, order_row.store_id, $2, \
                        'order.confirmed', contact.email, 'order_confirmation', 1, \
                        jsonb_build_object( \
                            'order_id', order_row.id, \
                            'order_number', order_row.order_number, \
                            'tracking_key', $5, \
                            'total_amount_minor', order_row.total_amount_minor, \
                            'currency', order_row.currency::text \
                        ), 'resend' \
                   FROM commerce.orders AS order_row \
                   INNER JOIN commerce.order_contacts AS contact \
                     ON contact.store_id = order_row.store_id \
                    AND contact.order_id = order_row.id \
                  WHERE order_row.store_id = $3 AND order_row.id = $4 \
                 ON CONFLICT (store_id, semantic_event_id) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(transition_id)
            .bind(store_id.as_uuid())
            .bind(order_id.as_uuid())
            .bind(tracking_key)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        idempotency::complete(
            &mut transaction,
            &IdempotencyScope::Store(store_id.as_uuid()),
            operation,
            request,
            200,
            json!({"id": order_id.as_uuid()}),
        )
        .await?;
        let detail = load_order(&mut transaction, store_id, order_id)
            .await?
            .ok_or_else(|| order_not_found(order_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
}

async fn load_order(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<Option<OrderDetail>, ApplicationError> {
    let row = sqlx::query_as::<_, HeaderRow>(
        "SELECT id, shopper_id, customer_id, checkout_id, inventory_reservation_id, price_list_id, currency::text, \
                status::text, subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                tax_inclusive, shipping_amount_minor, total_amount_minor, created_at, updated_at FROM commerce.orders \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let (locale, order_number) = sqlx::query_as::<_, (String, String)>(
        "SELECT locale, order_number FROM commerce.orders \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    let derived_statuses = sqlx::query_as::<_, (String, String)>(
        "SELECT fulfillment_status::text, delivery_status::text FROM commerce.orders \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    let identity = load_order_identity(transaction, store_id, order_id).await?;
    let shipping = load_order_shipping(transaction, store_id, order_id).await?;
    let tax_rule = load_order_tax(transaction, store_id, order_id).await?;
    let promotion = load_order_promotion(transaction, store_id, order_id).await?;
    let lines = sqlx::query_as::<_, LineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor, \
                subtotal_amount_minor, discount_amount_minor, tax_amount_minor, \
                total_amount_minor, tax_inclusive FROM commerce.order_lines \
         WHERE store_id = $1 AND order_id = $2 ORDER BY position",
    )
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
         FROM commerce.order_transitions WHERE store_id = $1 \
           AND order_id = $2 ORDER BY occurred_at, id",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(Some(OrderDetail {
        id: OrderId::from_uuid(row.0),
        order_number: OrderNumber::parse(order_number)?,
        shopper_id: ShopperId::from_uuid(row.1),
        customer_id: row.2.map(CustomerId::from_uuid),
        checkout_id: CheckoutId::from_uuid(row.3),
        inventory_reservation_id: row.4.map(InventoryReservationId::from_uuid),
        price_list_id: PriceListId::from_uuid(row.5),
        currency: CurrencyCode::parse(&row.6)?,
        locale: Locale::parse(&locale)?,
        status: OrderStatus::parse(&row.7).ok_or_else(corrupt_state)?,
        fulfillment_status: OrderFulfillmentStatus::parse(&derived_statuses.0)
            .ok_or_else(corrupt_state)?,
        delivery_status: OrderDeliveryStatus::parse(&derived_statuses.1)
            .ok_or_else(corrupt_state)?,
        identity,
        subtotal_amount_minor: row.8,
        discount_amount_minor: row.9,
        tax_amount_minor: row.10,
        tax_rule,
        promotion,
        tax_inclusive: row.11,
        shipping,
        shipping_amount_minor: row.12,
        total_amount_minor: row.13,
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
        created_at: row.14,
        updated_at: row.15,
    }))
}

async fn load_order_shipping(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<Option<ShippingSelection>, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, i64, String, i16, i16)>(
        "SELECT shipping_service_id, service_code, service_name, amount_minor, currency::text, \
                estimated_min_days, estimated_max_days \
         FROM commerce.order_shipping_selections \
         WHERE store_id = $1 AND order_id = $2",
    )
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

async fn load_order_tax(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<TaxRuleSnapshot, ApplicationError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String, i32)>(
        "SELECT tax_rule_id, rule_code, rule_name, country_code::text, rate_basis_points \
         FROM commerce.order_tax_calculations \
         WHERE store_id = $1 AND order_id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_state)?;
    TaxRuleSnapshot::rehydrate(
        TaxRuleId::from_uuid(row.0),
        row.1,
        row.2,
        row.3,
        u32::try_from(row.4).map_err(|error| ApplicationError::Unexpected(error.into()))?,
    )
    .map_err(ApplicationError::from)
}

async fn load_order_promotion(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<Option<PromotionSnapshot>, ApplicationError> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            String,
            Option<String>,
            String,
            Option<i32>,
            Option<i64>,
            Option<i64>,
            String,
            i64,
            i16,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
        ),
    >(
        "SELECT promotion_id, handle, name, trigger::text, redemption_code, value_kind::text, \
                rate_basis_points, amount_minor, maximum_amount_minor, currency::text, \
                minimum_subtotal_amount_minor, priority, starts_at, ends_at \
         FROM commerce.order_promotion_calculations \
         WHERE store_id = $1 AND order_id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        let value = match row.5.as_str() {
            "percentage" => PromotionValue::Percentage {
                rate_basis_points: u32::try_from(row.6.ok_or_else(corrupt_state)?)
                    .map_err(|error| ApplicationError::Unexpected(error.into()))?,
                maximum_amount_minor: row.8,
            },
            "fixed_amount" => PromotionValue::FixedAmount {
                amount_minor: row.7.ok_or_else(corrupt_state)?,
            },
            _ => return Err(corrupt_state()),
        };
        PromotionSnapshot::rehydrate(
            PromotionId::from_uuid(row.0),
            row.1,
            row.2,
            PromotionTrigger::parse(&row.3).ok_or_else(corrupt_state)?,
            row.4,
            value,
            CurrencyCode::parse(&row.9)?,
            row.10,
            u16::try_from(row.11).map_err(|error| ApplicationError::Unexpected(error.into()))?,
            row.12,
            row.13,
        )
        .map_err(ApplicationError::from)
    })
    .transpose()
}

async fn load_order_identity(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<CheckoutIdentity, ApplicationError> {
    let contact = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT email::text, phone FROM commerce.order_contacts \
         WHERE store_id = $1 AND order_id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_state)?;
    let addresses = sqlx::query_as::<_, AddressRow>(
        "SELECT kind::text, full_name, company, address_line1, address_line2, locality, \
                administrative_area, postal_code, country_code::text \
         FROM commerce.order_addresses \
         WHERE store_id = $1 AND order_id = $2 ORDER BY kind",
    )
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
