use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{AdminActor, IdempotencyRequest, MachineActor, ReviewRepository, ReviewSummary},
};
use chaos_domain::{
    catalog::{ProductId, ReviewId, ReviewStatus, StaffReplyContent},
    merchant::StoreId,
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const SUBMIT: &str = "reviews.submit.v1";
const APPROVE: &str = "reviews.approve.v1";
const REJECT: &str = "reviews.reject.v1";
const ADD_REPLY: &str = "reviews.add_reply.v1";

type ReviewRow = (
    Uuid,
    Option<Uuid>,
    Option<i16>,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
    bool,
    bool,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(Clone)]
pub struct PostgresReviewRepository {
    pool: PgPool,
}

impl PostgresReviewRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(actor.merchant_account_id().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        Ok(tx)
    }

    async fn begin_machine(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(actor.merchant_account_id.as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        Ok(tx)
    }
}

#[async_trait]
impl ReviewRepository for PostgresReviewRepository {
    async fn submit(
        &self,
        actor: &MachineActor,
        record: chaos_application::ports::SubmitReviewRecord,
        request: &IdempotencyRequest,
    ) -> Result<ReviewId, ApplicationError> {
        let mut tx = self.begin_machine(actor).await?;
        if let Some(id) = reserve_machine(&mut tx, actor, SUBMIT, request).await? {
            return Ok(ReviewId::from_uuid(id));
        }
        let content = record.content;
        sqlx::query(
            "INSERT INTO catalog.reviews \
             (id, merchant_account_id, store_id, product_id, rating, title, content, \
              author_name, author_email, status, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'pending',$10,$10)",
        )
        .bind(record.id.as_uuid())
        .bind(actor.merchant_account_id.as_uuid())
        .bind(record.store_id.as_uuid())
        .bind(record.product_id.as_uuid())
        .bind(i16::from(content.rating().value()))
        .bind(content.title())
        .bind(content.content())
        .bind(content.author_name())
        .bind(content.author_email().map(|email| email.as_str()))
        .bind(record.created_at)
        .execute(&mut *tx)
        .await
        .map_err(map_review_write_error)?;
        event(
            &mut tx,
            actor.merchant_account_id.as_uuid(),
            record.store_id,
            record.id,
            "submitted",
            None,
            record.created_at,
        )
        .await?;
        complete_machine(&mut tx, actor, SUBMIT, request, record.id, 201).await?;
        tx.commit().await.map_err(database_error)?;
        Ok(record.id)
    }

    async fn list_by_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        status: ReviewStatus,
        after: Option<ReviewId>,
        limit: u16,
    ) -> Result<Option<Vec<ReviewSummary>>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        if !store_exists(&mut tx, &actor, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                Option<Uuid>,
                Option<i16>,
                Option<String>,
                String,
                String,
                Option<String>,
                String,
                bool,
                bool,
                OffsetDateTime,
                OffsetDateTime,
                Uuid,
            ),
        >(
            "SELECT id, parent_review_id, rating, title, content, author_name, \
                    author_email::text, status::text, is_staff_reply, verified_buyer, \
                    created_at, updated_at, product_id \
             FROM catalog.reviews \
             WHERE merchant_account_id=$1 AND store_id=$2 AND status=$3::catalog.review_status \
               AND ($4::uuid IS NULL OR id < $4) \
             ORDER BY id DESC LIMIT $5",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(status.as_str())
        .bind(after.map(ReviewId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                let (
                    id,
                    parent_review_id,
                    rating,
                    title,
                    content,
                    author_name,
                    author_email,
                    status,
                    is_staff_reply,
                    verified_buyer,
                    created_at,
                    updated_at,
                    product_id,
                ) = row;
                row_to_summary(
                    (
                        id,
                        parent_review_id,
                        rating,
                        title,
                        content,
                        author_name,
                        author_email,
                        status,
                        is_staff_reply,
                        verified_buyer,
                        created_at,
                        updated_at,
                    ),
                    ProductId::from_uuid(product_id),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn set_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        review_id: ReviewId,
        status: ReviewStatus,
        verified_buyer: bool,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<ReviewId, ApplicationError> {
        let operation = if status == ReviewStatus::Approved {
            APPROVE
        } else {
            REJECT
        };
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, operation, request).await? {
            return Ok(ReviewId::from_uuid(id));
        }
        let approved = status == ReviewStatus::Approved;
        let changed = sqlx::query(
            "UPDATE catalog.reviews \
             SET status=$4::catalog.review_status, \
                 verified_buyer=$5, \
                 approved_at=CASE WHEN $4::catalog.review_status='approved' THEN $6 ELSE NULL END, \
                 approved_by_user_id=CASE WHEN $4::catalog.review_status='approved' THEN $7 ELSE NULL END, \
                 updated_at=$6 \
             WHERE merchant_account_id=$1 AND store_id=$2 AND id=$3 AND status='pending'",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(review_id.as_uuid())
        .bind(status.as_str())
        .bind(approved && verified_buyer)
        .bind(now)
        .bind(actor.audit_user_id().as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?
        .rows_affected();
        if changed == 0 && !review_exists(&mut tx, &actor, store_id, review_id).await? {
            return Err(not_found(review_id));
        }
        if changed == 1 {
            event(
                &mut tx,
                actor.merchant_account_id().as_uuid(),
                store_id,
                review_id,
                status.as_str(),
                Some(actor.audit_user_id().as_uuid()),
                now,
            )
            .await?;
        }
        complete(&mut tx, &actor, operation, request, review_id, 200).await?;
        tx.commit().await.map_err(database_error)?;
        Ok(review_id)
    }

    async fn add_reply(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        parent_review_id: ReviewId,
        content: StaffReplyContent,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<ReviewId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, ADD_REPLY, request).await? {
            return Ok(ReviewId::from_uuid(id));
        }
        let product_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT product_id FROM catalog.reviews \
             WHERE merchant_account_id=$1 AND store_id=$2 AND id=$3 AND status='approved' \
               AND parent_review_id IS NULL",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(parent_review_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        let Some(product_id) = product_id else {
            return Err(ApplicationError::Conflict {
                code: "review_not_repliable",
                message: "a reply requires an existing approved top-level review",
            });
        };
        let reply_id = ReviewId::new();
        sqlx::query(
            "INSERT INTO catalog.reviews \
             (id, merchant_account_id, store_id, product_id, parent_review_id, content, \
              author_name, status, is_staff_reply, approved_at, approved_by_user_id, \
              created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,'Altapano','approved',true,$7,$8,$7,$7)",
        )
        .bind(reply_id.as_uuid())
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id)
        .bind(parent_review_id.as_uuid())
        .bind(content.as_str())
        .bind(now)
        .bind(actor.audit_user_id().as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_review_write_error)?;
        event(
            &mut tx,
            actor.merchant_account_id().as_uuid(),
            store_id,
            reply_id,
            "reply_added",
            Some(actor.audit_user_id().as_uuid()),
            now,
        )
        .await?;
        complete(&mut tx, &actor, ADD_REPLY, request, reply_id, 201).await?;
        tx.commit().await.map_err(database_error)?;
        Ok(reply_id)
    }

    async fn list_approved_for_product(
        &self,
        actor: &MachineActor,
        product_id: ProductId,
        after: Option<ReviewId>,
        limit: u16,
    ) -> Result<Vec<ReviewSummary>, ApplicationError> {
        let mut tx = self.begin_machine(actor).await?;
        let top_level = sqlx::query_as::<_, ReviewRow>(
            "SELECT id, parent_review_id, rating, title, content, author_name, \
                    author_email::text, status::text, is_staff_reply, verified_buyer, \
                    created_at, updated_at \
             FROM catalog.reviews \
             WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 \
               AND status='approved' AND parent_review_id IS NULL \
               AND ($4::uuid IS NULL OR id < $4) \
             ORDER BY id DESC LIMIT $5",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(after.map(ReviewId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        let parent_ids: Vec<Uuid> = top_level.iter().map(|row| row.0).collect();
        let replies = if parent_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as::<_, ReviewRow>(
                "SELECT id, parent_review_id, rating, title, content, author_name, \
                        author_email::text, status::text, is_staff_reply, verified_buyer, \
                        created_at, updated_at \
                 FROM catalog.reviews \
                 WHERE merchant_account_id=$1 AND store_id=$2 AND status='approved' \
                   AND parent_review_id = ANY($3::uuid[]) \
                 ORDER BY parent_review_id, id ASC",
            )
            .bind(actor.merchant_account_id.as_uuid())
            .bind(actor.store_id.as_uuid())
            .bind(&parent_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(database_error)?
        };
        tx.commit().await.map_err(database_error)?;

        let mut result = Vec::with_capacity(top_level.len() + replies.len());
        for row in top_level {
            let id = row.0;
            result.push(row_to_summary(row, product_id)?);
            for reply in replies.iter().filter(|reply| reply.1 == Some(id)) {
                result.push(row_to_summary(reply.clone(), product_id)?);
            }
        }
        Ok(result)
    }
}

fn row_to_summary(
    row: ReviewRow,
    product_id: ProductId,
) -> Result<ReviewSummary, ApplicationError> {
    let (
        id,
        parent_review_id,
        rating,
        title,
        content,
        author_name,
        author_email,
        status,
        is_staff_reply,
        verified_buyer,
        created_at,
        updated_at,
    ) = row;
    Ok(ReviewSummary {
        id: ReviewId::from_uuid(id),
        product_id,
        parent_review_id: parent_review_id.map(ReviewId::from_uuid),
        rating: rating
            .map(u8::try_from)
            .transpose()
            .map_err(|_| invalid_snapshot())?,
        title,
        content,
        author_name,
        author_email,
        status: ReviewStatus::parse(&status).ok_or_else(invalid_snapshot)?,
        is_staff_reply,
        verified_buyer,
        created_at,
        updated_at,
    })
}

async fn store_exists(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM merchant.stores WHERE merchant_account_id=$1 AND id=$2 AND status<>'archived')",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)
}

async fn review_exists(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    id: ReviewId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM catalog.reviews WHERE merchant_account_id=$1 AND store_id=$2 AND id=$3)",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store.as_uuid())
    .bind(id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)
}

#[allow(clippy::too_many_arguments)]
async fn event(
    tx: &mut Transaction<'_, Postgres>,
    merchant_account_id: Uuid,
    store_id: StoreId,
    review_id: ReviewId,
    kind: &str,
    actor_user_id: Option<Uuid>,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO catalog.review_events \
         (id, merchant_account_id, store_id, review_id, event_kind, actor_user_id, occurred_at) \
         VALUES ($1,$2,$3,$4,$5::catalog.review_event_kind,$6,$7)",
    )
    .bind(Uuid::now_v7())
    .bind(merchant_account_id)
    .bind(store_id.as_uuid())
    .bind(review_id.as_uuid())
    .bind(kind)
    .bind(actor_user_id)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn reserve(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    reserve_for_scope(
        tx,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
    )
    .await
}

async fn reserve_machine(
    tx: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    reserve_for_scope(
        tx,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id.as_uuid()),
        operation,
        request,
    )
    .await
}

async fn reserve_for_scope(
    tx: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(value) = idempotency::reserve(tx, scope, operation, request).await? else {
        return Ok(None);
    };
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|v| Uuid::parse_str(v).ok())
        .map(Some)
        .ok_or_else(invalid_snapshot)
}

async fn complete(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: ReviewId,
    status: i16,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        tx,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
        status,
        json!({"id": id.as_uuid()}),
    )
    .await
}

async fn complete_machine(
    tx: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: ReviewId,
    status: i16,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        tx,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id.as_uuid()),
        operation,
        request,
        status,
        json!({"id": id.as_uuid()}),
    )
    .await
}

fn not_found(id: ReviewId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "review",
        id: id.as_uuid().to_string(),
    }
}

fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "the Review persistence snapshot is invalid"
    ))
}

fn map_review_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(db) = &error
        && db.is_foreign_key_violation()
    {
        return ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "product_id",
                reason: "must identify a Product in the same Store".into(),
            }],
        };
    }
    database_error(error)
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
