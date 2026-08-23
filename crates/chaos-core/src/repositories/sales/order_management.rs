use crate::{
    ApplicationError,
    error::database_error,
    ports::{AdminActor, OrderDetail, OrderListFilter, OrderPage},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_domain::{
    sales::{Order, OrderId, OrderStatus},
    store::StoreId,
};
use rand::Rng;
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
        crate::database::set_admin_context(
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
                super::order_detail::load(&mut transaction, store_id, None, OrderId::from_uuid(id))
                    .await?
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
        let audit_user_id = actor.audit_user_id().as_uuid();
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
