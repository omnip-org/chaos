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
    pricing::PriceListId,
    sales::{
        Order, OrderContact, OrderId, OrderIdentity, OrderNumber, OrderPaymentStatus,
        OrderShippingStatus, OrderStatus, PostalAddress, ShopperId,
    },
    store::StoreId,
};
use rand::Rng;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

const ORDER_TRACKING_TOKEN_LIFETIME: time::Duration = time::Duration::days(180);

fn generate_order_tracking_token() -> (String, [u8; 32]) {
    let mut secret = [0_u8; 32];
    rand::rng().fill_bytes(&mut secret);
    let plaintext = format!("ot_{}", URL_SAFE_NO_PAD.encode(secret));
    let digest = Sha256::digest(plaintext.as_bytes()).into();
    (plaintext, digest)
}

use crate::repositories::shared::idempotency::{self, IdempotencyScope};

const CONFIRM_OPERATION: &str = "orders.confirm.v1";
const CANCEL_OPERATION: &str = "orders.cancel.v1";

#[derive(sqlx::FromRow)]
struct HeaderRow {
    id: Uuid,
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
    stripe_checkout_session_id: Option<String>,
    stripe_payment_intent_id: Option<String>,
    stripe_charge_id: Option<String>,
    shipping_provider: Option<String>,
    shipping_provider_reference: Option<String>,
    shipping_tracking_number: Option<String>,
    shipping_tracking_url: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}
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
);
#[derive(sqlx::FromRow)]
struct InlineOrderIdentity {
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
             WHERE o.store_id = $1 \
               AND ($2::uuid IS NULL OR o.id < $2) \
               AND ($3::text IS NULL OR o.status::text = $3) \
               AND ($4::text IS NULL OR o.contact_email = lower($4)) \
               AND ($5::text IS NULL OR o.order_number = upper($5)) \
             ORDER BY o.id DESC LIMIT $6",
        )
        .bind(store_id.as_uuid())
        .bind(after)
        .bind(filter.status.map(OrderStatus::as_str))
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
        let row = sqlx::query_scalar::<_, String>(
            "SELECT status::text FROM commerce.orders \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        let current_status = OrderStatus::parse(&row).ok_or_else(corrupt_state)?;
        let mut order = Order::rehydrate(order_id, current_status);
        let transition = match target_status {
            OrderStatus::Confirmed => order.confirm(now)?,
            OrderStatus::Cancelled => order.cancel(now)?,
            OrderStatus::Pending => return Err(invalid_target()),
        };
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
            let (_, tracking_digest) = generate_order_tracking_token();
            sqlx::query(
                "INSERT INTO commerce.order_tracking_tokens \
                 (store_id,order_id,token_digest,expires_at,created_at) \
                 VALUES($1,$2,$3,$4,$5) ON CONFLICT(store_id,order_id) DO NOTHING",
            )
            .bind(store_id.as_uuid())
            .bind(order_id.as_uuid())
            .bind(tracking_digest.as_slice())
            .bind(now + ORDER_TRACKING_TOKEN_LIFETIME)
            .bind(now)
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
        "SELECT id, shopper_id, price_list_id, currency::text AS currency, \
                status::text AS status, payment_status::text AS payment_status, \
                shipping_status::text AS shipping_status, subtotal_amount_minor, \
                discount_amount_minor, tax_amount_minor, \
                shipping_amount_minor, total_amount_minor, refunded_amount_minor, \
                stripe_checkout_session_id, stripe_payment_intent_id, stripe_charge_id, \
                shipping_provider, shipping_provider_reference, shipping_tracking_number, \
                shipping_tracking_url, created_at, updated_at FROM commerce.orders \
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
    let identity = load_order_identity(transaction, store_id, order_id).await?;
    let lines = sqlx::query_as::<_, LineRow>(
        "SELECT product_id, product_variant_id, product_title, variant_title, sku, \
                requires_shipping, track_inventory, quantity, unit_price_amount_minor, \
                subtotal_amount_minor FROM commerce.order_lines \
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
        id: OrderId::from_uuid(row.id),
        order_number: OrderNumber::parse(order_number)?,
        shopper_id: ShopperId::from_uuid(row.shopper_id),
        price_list_id: PriceListId::from_uuid(row.price_list_id),
        currency: CurrencyCode::parse(&row.currency)?,
        locale: Locale::parse(&locale)?,
        status: OrderStatus::parse(&row.status).ok_or_else(corrupt_state)?,
        payment_status: OrderPaymentStatus::parse(&row.payment_status).ok_or_else(corrupt_state)?,
        shipping_status: OrderShippingStatus::parse(&row.shipping_status)
            .ok_or_else(corrupt_state)?,
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
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

async fn load_order_identity(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<OrderIdentity, ApplicationError> {
    let row = sqlx::query_as::<_, InlineOrderIdentity>(
        "SELECT contact_email::text, contact_phone, billing_full_name, billing_company, \
                billing_address_line1, billing_address_line2, billing_locality, \
                billing_administrative_area, billing_postal_code, billing_country_code::text, \
                shipping_full_name, shipping_company, shipping_address_line1, \
                shipping_address_line2, shipping_locality, shipping_administrative_area, \
                shipping_postal_code, shipping_country_code::text \
         FROM commerce.orders WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(corrupt_state)?;
    let address = |full_name: Option<String>,
                   company: Option<String>,
                   line1: Option<String>,
                   line2: Option<String>,
                   locality: Option<String>,
                   area: Option<String>,
                   postal: Option<String>,
                   country: Option<String>|
     -> Result<Option<PostalAddress>, ApplicationError> {
        match (full_name, line1, locality, country) {
            (None, None, None, None) => Ok(None),
            (Some(full_name), Some(line1), Some(locality), Some(country)) => {
                Ok(Some(PostalAddress::new(
                    full_name, company, line1, line2, locality, area, postal, country,
                )?))
            }
            _ => Err(corrupt_state()),
        }
    };
    Ok(OrderIdentity::new(
        OrderContact::new(
            row.contact_email.ok_or_else(corrupt_state)?,
            row.contact_phone,
        )?,
        address(
            row.billing_full_name,
            row.billing_company,
            row.billing_address_line1,
            row.billing_address_line2,
            row.billing_locality,
            row.billing_administrative_area,
            row.billing_postal_code,
            row.billing_country_code,
        )?,
        address(
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
