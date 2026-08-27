use std::collections::HashMap;

use crate::{
    ApplicationError,
    contracts::{
        AdminActor, CreateManualReviewRecord, MachineActor, ReviewMediaSummary, ReviewSummary,
    },
    error::database_error,
};
use chaos_domain::{
    catalog::{
        MediaAssetId, MediaAssetStatus, MediaKind, ProductId, ReviewId, ReviewOrigin, ReviewStatus,
        StaffReplyContent,
    },
    store::StoreId,
};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, FromRow)]
struct ReviewRow {
    id: Uuid,
    product_id: Uuid,
    parent_review_id: Option<Uuid>,
    rating: Option<i16>,
    title: Option<String>,
    content: String,
    author_name: String,
    author_email: Option<String>,
    status: String,
    is_staff_reply: bool,
    verified_buyer: bool,
    origin: String,
    source_channel: Option<String>,
    source_reference: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(FromRow)]
struct ReviewImageRow {
    review_id: Uuid,
    asset_id: Uuid,
    media_type: String,
    media_kind: String,
    alt_text: String,
    position: i16,
    status: String,
    public_url: Option<String>,
}

struct ReviewInsertRecord {
    id: ReviewId,
    store_id: StoreId,
    product_id: ProductId,
    content: chaos_domain::catalog::ReviewContent,
    origin: ReviewOrigin,
    source_channel: Option<String>,
    source_reference: Option<String>,
    publication_consent_confirmed: bool,
    created_by_user_id: Option<chaos_domain::identity::UserId>,
    created_at: OffsetDateTime,
}

#[derive(Clone, FromRow)]
struct PublicReviewRow {
    id: Uuid,
    product_id: Uuid,
    parent_review_id: Option<Uuid>,
    rating: Option<i16>,
    title: Option<String>,
    content: String,
    author_name: String,
    status: String,
    is_staff_reply: bool,
    verified_buyer: bool,
    origin: String,
    source_channel: Option<String>,
    source_reference: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

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
        crate::adapters::postgres::database::set_admin_context(
            &mut tx,
            actor.audit_user_id(),
            actor.store_id(),
        )
        .await
        .map_err(database_error)?;
        Ok(tx)
    }

    async fn begin_machine(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_store_context(&mut tx, actor.store_id)
            .await
            .map_err(database_error)?;
        Ok(tx)
    }
}

impl PostgresReviewRepository {
    pub(crate) async fn submit(
        &self,
        actor: &MachineActor,
        record: crate::contracts::SubmitReviewRecord,
    ) -> Result<ReviewId, ApplicationError> {
        let mut tx = self.begin_machine(actor).await?;
        insert_review(
            &mut tx,
            ReviewInsertRecord {
                id: record.id,
                store_id: record.store_id,
                product_id: record.product_id,
                content: record.content,
                origin: record.origin,
                source_channel: record.source_channel,
                source_reference: record.source_reference,
                publication_consent_confirmed: record.publication_consent_confirmed,
                created_by_user_id: record.created_by_user_id,
                created_at: record.created_at,
            },
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        Ok(record.id)
    }

    pub(crate) async fn create_manual(
        &self,
        actor: AdminActor,
        record: CreateManualReviewRecord,
    ) -> Result<ReviewId, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        insert_review(
            &mut tx,
            ReviewInsertRecord {
                id: record.id,
                store_id: record.store_id,
                product_id: record.product_id,
                content: record.content,
                origin: ReviewOrigin::Manual,
                source_channel: Some(record.source_channel),
                source_reference: record.source_reference,
                publication_consent_confirmed: record.publication_consent_confirmed,
                created_by_user_id: record.created_by_user_id,
                created_at: record.created_at,
            },
        )
        .await?;
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
        if !store_exists(&mut tx, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, ReviewRow>(
            "SELECT id, product_id, parent_review_id, rating, title, content, author_name, \
                    author_email::text, status::text, is_staff_reply, verified_buyer, \
                    origin::text, source_channel, source_reference, created_at, updated_at \
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
        let image_map = load_review_images(
            &mut tx,
            store_id,
            &rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                let images = image_map.get(&row.id).cloned().unwrap_or_default();
                row_to_summary(row, images)
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
        if !review_exists_for_update(&mut tx, store_id, review_id).await? {
            return Err(not_found(review_id));
        }
        let approved = status == ReviewStatus::Approved;
        if approved && review_has_unready_media(&mut tx, store_id, review_id).await? {
            return Err(ApplicationError::Conflict {
                code: "review_media_pending",
                message: "all review images must finish uploading before approval",
            });
        }
        sqlx::query(
            "UPDATE commerce.reviews \
             SET status=$3::commerce.review_status, \
                 verified_buyer=$4, \
                 approved_at=CASE WHEN $3::commerce.review_status='approved' THEN $5 ELSE NULL END, \
                 updated_at=$5 \
             WHERE store_id=$1 AND id=$2 AND status='pending'",
        )
        .bind(store_id.as_uuid())
        .bind(review_id.as_uuid())
        .bind(status.as_str())
        .bind(approved && verified_buyer)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?
        .rows_affected();
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
              author_name, status, is_staff_reply, approved_at, \
              created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,'Altapano','approved',true,$6,$6,$6)",
        )
        .bind(reply_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id)
        .bind(parent_review_id.as_uuid())
        .bind(content.as_str())
        .bind(now)
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
        let top_level = sqlx::query_as::<_, PublicReviewRow>(
            "SELECT id, product_id, parent_review_id, rating, title, content, author_name, \
	                status::text, is_staff_reply, verified_buyer, origin::text, \
	                source_channel, source_reference, created_at, updated_at \
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
        let parent_ids: Vec<Uuid> = top_level.iter().map(|row| row.id).collect();
        let replies = if parent_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as::<_, PublicReviewRow>(
                "SELECT id, product_id, parent_review_id, rating, title, content, author_name, \
	                    status::text, is_staff_reply, verified_buyer, origin::text, \
	                    source_channel, source_reference, created_at, updated_at \
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
        let all_ids = top_level
            .iter()
            .chain(replies.iter())
            .map(|row| row.id)
            .collect::<Vec<_>>();
        let image_map = load_review_images(&mut tx, actor.store_id, &all_ids).await?;
        tx.commit().await.map_err(database_error)?;

        let mut result = Vec::with_capacity(top_level.len() + replies.len());
        for row in top_level {
            let id = row.id;
            result.push(public_row_to_summary(
                row,
                image_map.get(&id).cloned().unwrap_or_default(),
            )?);
            for reply in replies
                .iter()
                .filter(|reply| reply.parent_review_id == Some(id))
            {
                result.push(public_row_to_summary(
                    reply.clone(),
                    image_map.get(&reply.id).cloned().unwrap_or_default(),
                )?);
            }
        }
        Ok(result)
    }
}

fn public_row_to_summary(
    row: PublicReviewRow,
    images: Vec<ReviewMediaSummary>,
) -> Result<ReviewSummary, ApplicationError> {
    row_to_summary(
        ReviewRow {
            id: row.id,
            product_id: row.product_id,
            parent_review_id: row.parent_review_id,
            rating: row.rating,
            title: row.title,
            content: row.content,
            author_name: row.author_name,
            // Storefront reads intentionally never select the submitted email.
            author_email: None,
            status: row.status,
            is_staff_reply: row.is_staff_reply,
            verified_buyer: row.verified_buyer,
            origin: row.origin,
            source_channel: row.source_channel,
            source_reference: row.source_reference,
            created_at: row.created_at,
            updated_at: row.updated_at,
        },
        images,
    )
}

async fn insert_review(
    tx: &mut Transaction<'_, Postgres>,
    record: ReviewInsertRecord,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.reviews \
         (id, store_id, product_id, rating, title, content, author_name, author_email, \
          status, is_staff_reply, verified_buyer, origin, source_channel, source_reference, \
          publication_consent_confirmed, created_by_user_id, created_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending',false,false,$9::commerce.review_origin,\
                 $10,$11,$12,$13,$14,$14)",
    )
    .bind(record.id.as_uuid())
    .bind(record.store_id.as_uuid())
    .bind(record.product_id.as_uuid())
    .bind(i16::from(record.content.rating().value()))
    .bind(record.content.title())
    .bind(record.content.content())
    .bind(record.content.author_name())
    .bind(record.content.author_email().map(|email| email.as_str()))
    .bind(record.origin.as_str())
    .bind(record.source_channel)
    .bind(record.source_reference)
    .bind(record.publication_consent_confirmed)
    .bind(record.created_by_user_id.map(|id| id.as_uuid()))
    .bind(record.created_at)
    .execute(&mut **tx)
    .await
    .map_err(map_review_write_error)?;
    Ok(())
}

async fn load_review_images(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    review_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<ReviewMediaSummary>>, ApplicationError> {
    if review_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, ReviewImageRow>(
        "SELECT link.review_id, media.id AS asset_id, media.media_type, \
                media.media_kind::text, link.alt_text, link.position, \
                media.status::text, media.public_url \
         FROM commerce.review_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         WHERE link.store_id=$1 AND link.review_id = ANY($2::uuid[]) \
           AND link.archived_at IS NULL \
         ORDER BY link.review_id, link.position, media.id",
    )
    .bind(store_id.as_uuid())
    .bind(review_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?;
    let mut result: HashMap<Uuid, Vec<ReviewMediaSummary>> = HashMap::new();
    for row in rows {
        let kind = match row.media_kind.as_str() {
            "image" => MediaKind::Image,
            "video" => MediaKind::Video,
            _ => return Err(invalid_snapshot()),
        };
        let status = MediaAssetStatus::parse(&row.status).ok_or_else(invalid_snapshot)?;
        let position = u16::try_from(row.position).map_err(|_| invalid_snapshot())?;
        result
            .entry(row.review_id)
            .or_default()
            .push(ReviewMediaSummary {
                id: MediaAssetId::from_uuid(row.asset_id),
                media_type: row.media_type,
                kind,
                alt_text: row.alt_text,
                position,
                status,
                public_url: row.public_url,
            });
    }
    Ok(result)
}

async fn review_has_unready_media(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    review_id: ReviewId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar(
        "SELECT EXISTS( \
            SELECT 1 \
            FROM commerce.review_media_assets AS link \
            INNER JOIN commerce.media_assets AS media \
                ON media.store_id=link.store_id AND media.id=link.media_asset_id \
            WHERE link.store_id=$1 AND link.review_id=$2 \
              AND link.archived_at IS NULL AND media.status <> 'ready' \
        )",
    )
    .bind(store_id.as_uuid())
    .bind(review_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)
}

fn row_to_summary(
    row: ReviewRow,
    images: Vec<ReviewMediaSummary>,
) -> Result<ReviewSummary, ApplicationError> {
    let origin = ReviewOrigin::parse(&row.origin).ok_or_else(invalid_snapshot)?;
    Ok(ReviewSummary {
        id: ReviewId::from_uuid(row.id),
        product_id: ProductId::from_uuid(row.product_id),
        parent_review_id: row.parent_review_id.map(ReviewId::from_uuid),
        rating: row
            .rating
            .map(u8::try_from)
            .transpose()
            .map_err(|_| invalid_snapshot())?,
        title: row.title,
        content: row.content,
        author_name: row.author_name,
        author_email: row.author_email,
        status: ReviewStatus::parse(&row.status).ok_or_else(invalid_snapshot)?,
        is_staff_reply: row.is_staff_reply,
        verified_buyer: row.verified_buyer,
        origin,
        source_channel: row.source_channel,
        source_reference: row.source_reference,
        images,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn store_exists(
    tx: &mut Transaction<'_, Postgres>,
    store: StoreId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commerce.stores WHERE id=$1)")
        .bind(store.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(database_error)
}

async fn review_exists_for_update(
    tx: &mut Transaction<'_, Postgres>,
    store: StoreId,
    id: ReviewId,
) -> Result<bool, ApplicationError> {
    sqlx::query("SELECT 1 FROM commerce.reviews WHERE store_id=$1 AND id=$2 FOR UPDATE")
        .bind(store.as_uuid())
        .bind(id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map(|row| row.is_some())
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
        && db.constraint() == Some("reviews_store_id_product_fkey")
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
