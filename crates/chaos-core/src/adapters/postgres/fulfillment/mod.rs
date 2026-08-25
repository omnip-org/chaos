//! Fulfillment tracking: shipping provider accounts and per-Order shipments.

use crate::{
    ApplicationError,
    contracts::{AdminActor, FulfillmentDetail, ShippingProviderAccountDetail},
    error::database_error,
};
use chaos_domain::{
    fulfillment::{Fulfillment, FulfillmentId, FulfillmentStatus, ShippingProviderAccountId},
    integration::ShippingProvider,
    sales::OrderId,
    store::StoreId,
};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct ShippingProviderAccountRow {
    id: Uuid,
    provider: String,
    display_name: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct FulfillmentRow {
    id: Uuid,
    order_id: Uuid,
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

#[derive(Clone)]
pub struct PostgresFulfillmentRepository {
    pool: PgPool,
}

impl PostgresFulfillmentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_admin(
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

    pub(crate) async fn list_shipping_provider_accounts(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Vec<ShippingProviderAccountDetail>, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        let rows = sqlx::query_as::<_, ShippingProviderAccountRow>(
            "SELECT id, provider::text, display_name, created_at, updated_at \
             FROM integration.provider_accounts \
             WHERE store_id = $1 AND capability = 'shipping' ORDER BY created_at, id",
        )
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(shipping_provider_account_detail)
            .collect::<Result<Vec<_>, _>>()
    }

    pub(crate) async fn create_fulfillment(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
        shipping_provider_account_id: ShippingProviderAccountId,
        tracking_number: Option<String>,
        tracking_url: Option<String>,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        let order_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce.orders WHERE store_id = $1 AND id = $2)",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !order_exists {
            return Err(order_not_found(order_id));
        }
        let account_exists: Option<String> = sqlx::query_scalar(
            "SELECT provider::text FROM integration.provider_accounts \
             WHERE store_id = $1 AND id = $2 AND capability = 'shipping'",
        )
        .bind(store_id.as_uuid())
        .bind(shipping_provider_account_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        account_exists
            .ok_or_else(|| shipping_provider_account_not_found(shipping_provider_account_id))?;
        let fulfillment = Fulfillment::create(
            order_id,
            shipping_provider_account_id,
            tracking_number,
            tracking_url,
        )?;
        sqlx::query(
            "INSERT INTO commerce.fulfillments \
             (id, store_id, order_id, shipping_provider_account_id, status, \
              tracking_number, tracking_url) \
             VALUES ($1, $2, $3, $4, 'awaiting_pickup', $5, $6)",
        )
        .bind(fulfillment.id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(shipping_provider_account_id.as_uuid())
        .bind(fulfillment.tracking_number())
        .bind(fulfillment.tracking_url())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let order_provider_bound = sqlx::query(
            "UPDATE commerce.orders \
             SET shipping_provider_account_id = $3, \
                 updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2 \
               AND (shipping_provider_account_id IS NULL \
                    OR shipping_provider_account_id = $3)",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(shipping_provider_account_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if order_provider_bound.rows_affected() != 1 {
            return Err(ApplicationError::Conflict {
                code: "shipping_provider_mismatch",
                message: "the Order is already assigned to another shipping provider account",
            });
        }
        recompute_order_shipping_status(&mut transaction, store_id, order_id).await?;
        let detail = load_fulfillment(&mut transaction, store_id, fulfillment.id())
            .await?
            .ok_or_else(|| fulfillment_not_found(fulfillment.id()))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn mark_shipped(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        id: FulfillmentId,
        tracking_number: Option<String>,
        tracking_url: Option<String>,
        now: OffsetDateTime,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        let mut fulfillment = load_domain_fulfillment(&mut transaction, store_id, id).await?;
        fulfillment.mark_shipped(tracking_number, tracking_url)?;
        sqlx::query(
            "UPDATE commerce.fulfillments \
                SET status = 'shipped', tracking_number = $3, tracking_url = $4, \
                    shipped_at = $5, updated_at = $5 \
              WHERE store_id = $1 AND id = $2 AND status = 'awaiting_pickup'",
        )
        .bind(store_id.as_uuid())
        .bind(id.as_uuid())
        .bind(fulfillment.tracking_number())
        .bind(fulfillment.tracking_url())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let order_id = fulfillment.order_id();
        sqlx::query(
            "INSERT INTO integration.event_outbox \
             (id, store_id, aggregate_type, aggregate_id, internal_event_type, payload) \
             VALUES ($1, $2, 'fulfillment', $3, 'fulfillment.shipped', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(store_id.as_uuid())
        .bind(id.as_uuid())
        .bind(serde_json::json!({
            "aggregate_id": id.as_uuid(),
            "fulfillment_id": id.as_uuid(),
            "order_id": order_id.as_uuid(),
            "shipping_provider_account_id": fulfillment.shipping_provider_account_id().as_uuid(),
            "tracking_number": fulfillment.tracking_number(),
            "tracking_url": fulfillment.tracking_url(),
            "operation": "shipped",
        }))
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        recompute_order_shipping_status(&mut transaction, store_id, order_id).await?;
        let detail = load_fulfillment(&mut transaction, store_id, id)
            .await?
            .ok_or_else(|| fulfillment_not_found(id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn mark_delivered(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        id: FulfillmentId,
        now: OffsetDateTime,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        let mut fulfillment = load_domain_fulfillment(&mut transaction, store_id, id).await?;
        fulfillment.mark_delivered()?;
        sqlx::query(
            "UPDATE commerce.fulfillments \
                SET status = 'delivered', delivered_at = $3, updated_at = $3 \
              WHERE store_id = $1 AND id = $2 AND status = 'shipped'",
        )
        .bind(store_id.as_uuid())
        .bind(id.as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let order_id = fulfillment.order_id();
        recompute_order_shipping_status(&mut transaction, store_id, order_id).await?;
        let detail = load_fulfillment(&mut transaction, store_id, id)
            .await?
            .ok_or_else(|| fulfillment_not_found(id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn cancel(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        id: FulfillmentId,
        now: OffsetDateTime,
    ) -> Result<FulfillmentDetail, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        let mut fulfillment = load_domain_fulfillment(&mut transaction, store_id, id).await?;
        fulfillment.cancel()?;
        sqlx::query(
            "UPDATE commerce.fulfillments \
                SET status = 'cancelled', cancelled_at = $3, updated_at = $3 \
              WHERE store_id = $1 AND id = $2 AND status IN ('awaiting_pickup', 'shipped')",
        )
        .bind(store_id.as_uuid())
        .bind(id.as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let order_id = fulfillment.order_id();
        recompute_order_shipping_status(&mut transaction, store_id, order_id).await?;
        let detail = load_fulfillment(&mut transaction, store_id, id)
            .await?
            .ok_or_else(|| fulfillment_not_found(id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }
}

/// A Store's `orders.shipping_status` is a read projection of its
/// Fulfillments, not an independent source of truth. Recomputing it from the
/// current Fulfillment rows (rather than patching it incrementally) keeps it
/// correct under concurrent or out-of-order Fulfillment writes.
///
/// An Order may have several concurrently active (non-cancelled)
/// Fulfillments — split shipments are normal, not an error — so the
/// projection takes the weakest link across them: `awaiting_pickup` <
/// `shipped` < `delivered`. The Order is not `delivered` until every active
/// Fulfillment is delivered, but becomes `shipped` as soon as any of them
/// has moved past `awaiting_pickup`.
async fn recompute_order_shipping_status(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    order_id: OrderId,
) -> Result<(), ApplicationError> {
    let active_statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status::text FROM commerce.fulfillments \
         WHERE store_id = $1 AND order_id = $2 AND status <> 'cancelled'",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let shipping_status = if active_statuses.is_empty() {
        "pending"
    } else if active_statuses.iter().all(|status| status == "delivered") {
        "delivered"
    } else if active_statuses
        .iter()
        .any(|status| status == "shipped" || status == "delivered")
    {
        "shipped"
    } else {
        "awaiting_pickup"
    };
    sqlx::query(
        "UPDATE commerce.orders SET shipping_status = $3::commerce.order_shipping_status \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(order_id.as_uuid())
    .bind(shipping_status)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn load_domain_fulfillment(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    id: FulfillmentId,
) -> Result<Fulfillment, ApplicationError> {
    let row = sqlx::query_as::<_, FulfillmentRow>(
        "SELECT id, order_id, shipping_provider_account_id, status::text, tracking_number, \
                tracking_url, shipped_at, delivered_at, cancelled_at, created_at, updated_at \
         FROM commerce.fulfillments WHERE store_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or_else(|| fulfillment_not_found(id))?;
    Ok(Fulfillment::rehydrate(
        FulfillmentId::from_uuid(row.id),
        OrderId::from_uuid(row.order_id),
        ShippingProviderAccountId::from_uuid(row.shipping_provider_account_id),
        FulfillmentStatus::parse(&row.status).ok_or_else(corrupt_state)?,
        row.tracking_number,
        row.tracking_url,
    ))
}

async fn load_fulfillment(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    id: FulfillmentId,
) -> Result<Option<FulfillmentDetail>, ApplicationError> {
    sqlx::query_as::<_, FulfillmentRow>(
        "SELECT id, order_id, shipping_provider_account_id, status::text, tracking_number, \
                tracking_url, shipped_at, delivered_at, cancelled_at, created_at, updated_at \
         FROM commerce.fulfillments WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(fulfillment_detail)
    .transpose()
}

fn fulfillment_detail(row: FulfillmentRow) -> Result<FulfillmentDetail, ApplicationError> {
    Ok(FulfillmentDetail {
        id: FulfillmentId::from_uuid(row.id),
        order_id: OrderId::from_uuid(row.order_id),
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

fn shipping_provider_account_detail(
    row: ShippingProviderAccountRow,
) -> Result<ShippingProviderAccountDetail, ApplicationError> {
    Ok(ShippingProviderAccountDetail {
        id: ShippingProviderAccountId::from_uuid(row.id),
        provider: ShippingProvider::parse(&row.provider).ok_or_else(corrupt_state)?,
        display_name: row.display_name,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn fulfillment_not_found(id: FulfillmentId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "fulfillment",
        id: id.as_uuid().to_string(),
    }
}

fn shipping_provider_account_not_found(id: ShippingProviderAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "shipping_provider_account",
        id: id.as_uuid().to_string(),
    }
}

fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains an unknown Fulfillment state"
    ))
}
