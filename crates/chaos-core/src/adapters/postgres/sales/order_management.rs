use crate::{
    ApplicationError,
    adapters::postgres::{
        database::{ORDER_TRACKING_TOKEN_LIFETIME, generate_order_tracking_capability},
        sales::{consume_order_inventory, release_order_inventory},
    },
    contracts::{AdminActor, OrderDetail, OrderListFilter, OrderPage},
    error::database_error,
};
use chaos_domain::{
    sales::{Order, OrderId, OrderStatus},
    store::StoreId,
};
use secrecy::ExposeSecret;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

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
        crate::adapters::postgres::database::set_admin_context(
            &mut transaction,
            actor.audit_user_id(),
            actor.store_id(),
        )
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }
}

impl PostgresOrderManagementRepository {
    pub(crate) async fn list_orders(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<Uuid>,
        limit: u16,
        filter: &OrderListFilter,
    ) -> Result<OrderPage, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT o.id FROM commerce.orders o \
             WHERE o.store_id = $1 \
               AND ($2::uuid IS NULL OR o.id < $2) \
               AND ($3::text IS NULL OR o.status = $3::commerce.order_status) \
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
        let page_ids: Vec<Uuid> = ids.into_iter().take(usize::from(limit)).collect();
        let mut details =
            super::order_detail::load_many(&mut transaction, store_id, None, &page_ids).await?;
        for id in page_ids {
            items.push(
                details
                    .remove(&id)
                    .ok_or_else(|| order_not_found(OrderId::from_uuid(id)))?,
            );
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(OrderPage { items, has_more })
    }

    pub(crate) async fn get_order(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
    ) -> Result<Option<OrderDetail>, ApplicationError> {
        let mut transaction = self.begin_for_admin(&actor).await?;
        let detail = super::order_detail::load(&mut transaction, store_id, None, order_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn transition_order(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
        target_status: OrderStatus,
        now: OffsetDateTime,
    ) -> Result<OrderDetail, ApplicationError> {
        if target_status == OrderStatus::Pending {
            return Err(invalid_target());
        }
        let mut transaction = self.begin_for_admin(&actor).await?;
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
        match target_status {
            OrderStatus::Confirmed => {
                order.confirm()?;
                consume_order_inventory(&mut transaction, store_id.as_uuid(), order_id.as_uuid())
                    .await?;
            }
            OrderStatus::Cancelled => {
                order.cancel()?;
                release_order_inventory(&mut transaction, store_id.as_uuid(), order_id.as_uuid())
                    .await?;
            }
            OrderStatus::Pending => return Err(invalid_target()),
        };
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
        // A manual terminal transition also closes the provider-form
        // handoff. The handoff is Cart-owned operational state, not an Order
        // field, so every terminal Order path must clear it explicitly.
        sqlx::query(
            "UPDATE commerce.carts AS cart SET status = $3::commerce.cart_status, \
                    payment_client_action = NULL, updated_at = $4 \
             FROM commerce.orders AS sales_order \
             WHERE sales_order.store_id = $1 AND sales_order.id = $2 \
               AND cart.store_id = sales_order.store_id AND cart.id = sales_order.cart_id",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(if target_status == OrderStatus::Confirmed {
            "completed"
        } else {
            "abandoned"
        })
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if target_status == OrderStatus::Confirmed {
            let tracking_capability = generate_order_tracking_capability();
            sqlx::query(
                "INSERT INTO commerce.order_tracking_tokens \
                 (store_id, order_id, token_digest, expires_at, created_at) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (store_id, order_id) DO NOTHING",
            )
            .bind(store_id.as_uuid())
            .bind(order_id.as_uuid())
            .bind(tracking_capability.digest.as_slice())
            .bind(now + ORDER_TRACKING_TOKEN_LIFETIME)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO integration.event_outbox \
                 (id, store_id, aggregate_type, aggregate_id, internal_event_type, payload) \
                 VALUES ($1, $2, 'order', $3, 'order.confirmed', $4)",
            )
            .bind(Uuid::now_v7())
            .bind(store_id.as_uuid())
            .bind(order_id.as_uuid())
            .bind(serde_json::json!({
                "aggregate_id": order_id.as_uuid(),
                "order_id": order_id.as_uuid(),
                "tracking_token": tracking_capability.token.expose_secret(),
            }))
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
        let detail = super::order_detail::load(&mut transaction, store_id, None, order_id)
            .await?
            .ok_or_else(|| order_not_found(order_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
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
