use crate::{
    ApplicationError,
    error::database_error,
    ports::{AdminActor, MachineActor, ReviewSummary},
};
use chaos_domain::{
    catalog::{ProductId, ReviewId, ReviewStatus, StaffReplyContent},
    store::StoreId,
};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

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
        crate::database::set_admin_context(&mut tx, actor.audit_user_id(), actor.store_id())
            .await
            .map_err(database_error)?;
        Ok(tx)
    }

    async fn begin_machine(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        crate::database::set_store_context(&mut tx, actor.store_id)
            .await
            .map_err(database_error)?;
        Ok(tx)
    }
}

impl PostgresReviewRepository {
    pub(crate) async fn submit(
        &self,
        actor: &MachineActor,
        record: crate::ports::SubmitReviewRecord,
    ) -> Result<ReviewId, ApplicationError> {
        let mut tx = self.begin_machine(actor).await?;
        let content = record.content;
        sqlx::query(
            "INSERT INTO commerce.reviews \
             (id, store_id, product_id, rating, title, content, \
              author_name, author_email, status, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending',$9,$9)",
        )
        .bind(record.id.as_uuid())
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
        tx.commit().await.map_err(database_error)?;
        Ok(record.id)
    }

    pub(crate) async fn list_by_status(
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
             FROM commerce.reviews \
             WHERE store_id=$1 AND status=$2::commerce.review_status \
               AND ($3::uuid IS NULL OR id < $3) \
             ORDER BY id DESC LIMIT $4",
        )
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

    pub(crate) async fn set_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        review_id: ReviewId,
        status: ReviewStatus,
        verified_buyer: bool,
        now: OffsetDateTime,
    ) -> Result<ReviewId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let approved = status == ReviewStatus::Approved;
        let changed = sqlx::query(
            "UPDATE commerce.reviews \
             SET status=$3::commerce.review_status, \
                 verified_buyer=$4, \
                 approved_at=CASE WHEN $3::commerce.review_status='approved' THEN $5 ELSE NULL END, \
                 approved_by_user_id=CASE WHEN $3::commerce.review_status='approved' THEN $6 ELSE NULL END, \
                 updated_at=$5 \
             WHERE store_id=$1 AND id=$2 AND status='pending'",
        )
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
        if changed == 0 && !review_exists(&mut tx, store_id, review_id).await? {
            return Err(not_found(review_id));
        }
        tx.commit().await.map_err(database_error)?;
        Ok(review_id)
    }

    pub(crate) async fn add_reply(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        parent_review_id: ReviewId,
        content: StaffReplyContent,
        now: OffsetDateTime,
    ) -> Result<ReviewId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let product_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT product_id FROM commerce.reviews \
             WHERE store_id=$1 AND id=$2 AND status='approved' \
               AND parent_review_id IS NULL",
        )
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
            "INSERT INTO commerce.reviews \
             (id, store_id, product_id, parent_review_id, content, \
              author_name, status, is_staff_reply, approved_at, approved_by_user_id, \
              created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,'Altapano','approved',true,$6,$7,$6,$6)",
        )
        .bind(reply_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id)
        .bind(parent_review_id.as_uuid())
        .bind(content.as_str())
        .bind(now)
        .bind(actor.audit_user_id().as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(map_review_write_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(reply_id)
    }

    pub(crate) async fn list_approved_for_product(
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
             FROM commerce.reviews \
             WHERE store_id=$1 AND product_id=$2 \
               AND status='approved' AND parent_review_id IS NULL \
               AND ($3::uuid IS NULL OR id < $3) \
             ORDER BY id DESC LIMIT $4",
        )
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
                 FROM commerce.reviews \
                 WHERE store_id=$1 AND status='approved' \
                   AND parent_review_id = ANY($2::uuid[]) \
                 ORDER BY parent_review_id, id ASC",
            )
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
    _actor: &AdminActor,
    store: StoreId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commerce.stores WHERE id=$1)")
        .bind(store.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)
}

async fn review_exists(
    tx: &mut Transaction<'_, Postgres>,
    store: StoreId,
    id: ReviewId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commerce.reviews WHERE store_id=$1 AND id=$2)")
        .bind(store.as_uuid())
        .bind(id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)
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
