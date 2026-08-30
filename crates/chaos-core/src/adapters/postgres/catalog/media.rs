use std::collections::HashSet;

use crate::{
    ApplicationError,
    catalog::parse_json_pointer,
    contracts::{
        AdminActor, CreateMediaAssetRecord, MediaAssetItem, MediaAssetMutation,
        MediaAssetStorageRecord, ProductMediaAssetItem, ProductMediaAssetLinkRecord,
        ProductMediaAssetMutation, ProductMediaScope, ProductMetaMediaAssetItem,
        ProductMetaMediaAssetLinkRecord, ProductMetaMediaAssetMutation,
        ProductOptionValueMediaAssetLinkRecord, ProductOptionValueMediaAssetMutation,
        ProductVariantMediaAssetLinkRecord, ProductVariantMediaAssetMutation, ReviewMediaAssetItem,
        ReviewMediaAssetLinkRecord, ReviewMediaAssetMutation,
    },
    error::database_error,
};
use chaos_domain::{
    FieldViolation,
    catalog::{
        MediaAssetId, MediaAssetStatus, MediaKind, ProductId, ProductOptionId,
        ProductOptionValueId, ProductVariantId, ReviewId,
    },
    store::StoreId,
};
use serde_json::{Map, Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresMediaAssetRepository {
    pool: PgPool,
}

impl PostgresMediaAssetRepository {
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
}

#[derive(FromRow)]
struct MediaRow {
    id: Uuid,
    store_id: Uuid,
    object_key: String,
    file_name: String,
    media_type: String,
    media_kind: String,
    byte_size: i64,
    sha256_digest: Vec<u8>,
    status: String,
    public_url: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[derive(FromRow)]
struct ProductMediaRow {
    asset_id: Uuid,
    store_id: Uuid,
    product_id: Uuid,
    scope: String,
    option_id: Option<Uuid>,
    option_value_id: Option<Uuid>,
    product_variant_id: Option<Uuid>,
    file_name: String,
    media_type: String,
    media_kind: String,
    byte_size: i64,
    sha256_digest: Vec<u8>,
    status: String,
    public_url: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    alt_text: String,
    position: i16,
    link_archived_at: Option<OffsetDateTime>,
    object_key: String,
}

#[derive(FromRow)]
struct ReviewMediaRow {
    asset_id: Uuid,
    store_id: Uuid,
    review_id: Uuid,
    file_name: String,
    media_type: String,
    media_kind: String,
    byte_size: i64,
    sha256_digest: Vec<u8>,
    status: String,
    public_url: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    alt_text: String,
    position: i16,
    link_archived_at: Option<OffsetDateTime>,
    object_key: String,
}

#[derive(FromRow)]
struct ProductMetaMediaRow {
    asset_id: Uuid,
    store_id: Uuid,
    product_id: Uuid,
    meta_path: String,
    file_name: String,
    media_type: String,
    media_kind: String,
    byte_size: i64,
    sha256_digest: Vec<u8>,
    status: String,
    public_url: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    alt_text: String,
    link_archived_at: Option<OffsetDateTime>,
    object_key: String,
}

impl PostgresMediaAssetRepository {
    pub(crate) async fn create_asset(
        &self,
        actor: AdminActor,
        record: CreateMediaAssetRecord,
    ) -> Result<MediaAssetStorageRecord, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        insert_asset(&mut tx, &record).await?;
        let row = load_asset(&mut tx, record.store_id, record.id)
            .await?
            .ok_or_else(invalid_snapshot)?;
        tx.commit().await.map_err(database_error)?;
        storage_record(row)
    }

    pub(crate) async fn asset(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        media_asset_id: MediaAssetId,
    ) -> Result<MediaAssetStorageRecord, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let row = load_asset(&mut tx, store_id, media_asset_id)
            .await?
            .ok_or_else(|| not_found(media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        storage_record(row)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn list_assets(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<MediaAssetId>,
        limit: u16,
        status: Option<MediaAssetStatus>,
        kind: Option<MediaKind>,
        sha256_hex: Option<&str>,
        file_name: Option<&str>,
    ) -> Result<Vec<MediaAssetItem>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let digest = sha256_hex.map(decode_digest).transpose()?;
        let rows = sqlx::query_as::<_, MediaRow>(
            "SELECT id, store_id, object_key, file_name, media_type, media_kind::text AS media_kind, \
                    byte_size, sha256_digest, status::text AS status, public_url, \
                    created_at, updated_at \
             FROM commerce.media_assets \
             WHERE store_id=$1 \
               AND ($2::uuid IS NULL OR id > $2) \
               AND ($3::text IS NULL OR status::text = $3) \
               AND ($4::text IS NULL OR media_kind::text = $4) \
               AND ($5::bytea IS NULL OR sha256_digest = $5) \
               AND ($6::text IS NULL OR file_name ILIKE '%' || $6 || '%') \
             ORDER BY id ASC \
             LIMIT $7",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(MediaAssetId::as_uuid))
        .bind(status.map(MediaAssetStatus::as_str))
        .bind(kind.map(MediaKind::as_str))
        .bind(digest.as_ref().map(|value| value.as_slice()))
        .bind(file_name)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter().map(item).collect()
    }

    pub(crate) async fn list_product(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Option<Vec<ProductMediaAssetItem>>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce.products WHERE store_id=$1 AND id=$2)",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if !exists {
            return Ok(None);
        }
        let rows = product_media_rows(&mut tx, store_id, product_id).await?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(product_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub(crate) async fn list_product_option_value(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        option_id: ProductOptionId,
        option_value_id: ProductOptionValueId,
    ) -> Result<Option<Vec<ProductMediaAssetItem>>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let valid = ensure_product_option_value(
            &mut tx,
            store_id,
            product_id,
            option_id,
            option_value_id,
            false,
        )
        .await?;
        if !valid {
            return Ok(None);
        }
        let rows = product_media_rows(&mut tx, store_id, product_id)
            .await?
            .into_iter()
            .filter(|row| {
                row.option_id == Some(option_id.as_uuid())
                    && row.option_value_id == Some(option_value_id.as_uuid())
            })
            .collect::<Vec<_>>();
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(product_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub(crate) async fn list_product_variant(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        product_variant_id: ProductVariantId,
    ) -> Result<Option<Vec<ProductMediaAssetItem>>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let valid =
            ensure_product_variant(&mut tx, store_id, product_id, product_variant_id, false)
                .await?;
        if !valid {
            return Ok(None);
        }
        let rows = product_media_rows(&mut tx, store_id, product_id)
            .await?
            .into_iter()
            .filter(|row| row.product_variant_id == Some(product_variant_id.as_uuid()))
            .collect::<Vec<_>>();
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(product_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub(crate) async fn list_review(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        review_id: ReviewId,
    ) -> Result<Option<Vec<ReviewMediaAssetItem>>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce.reviews WHERE store_id=$1 AND id=$2)",
        )
        .bind(store_id.as_uuid())
        .bind(review_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if !exists {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, ReviewMediaRow>(
            "SELECT media.id AS asset_id, media.store_id, link.review_id, media.file_name, \
                    media.media_type, media.media_kind::text, media.byte_size, \
                    media.sha256_digest, media.status::text, media.public_url, \
                    media.created_at, media.updated_at, link.alt_text, link.position, \
                    link.archived_at AS link_archived_at, media.object_key \
             FROM commerce.review_media_assets AS link \
             INNER JOIN commerce.media_assets AS media \
                ON media.store_id=link.store_id AND media.id=link.media_asset_id \
             WHERE link.store_id=$1 AND link.review_id=$2 \
             ORDER BY link.position, media.id",
        )
        .bind(store_id.as_uuid())
        .bind(review_id.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(review_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub(crate) async fn list_product_meta(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Option<Vec<ProductMetaMediaAssetItem>>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM commerce.products WHERE store_id=$1 AND id=$2)",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if !exists {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, ProductMetaMediaRow>(
            "SELECT media.id AS asset_id, media.store_id, link.product_id, link.meta_path, \
                    media.file_name, media.media_type, media.media_kind::text, media.byte_size, \
                    media.sha256_digest, media.status::text, media.public_url, \
                    media.created_at, media.updated_at, link.alt_text, \
                    link.archived_at AS link_archived_at, media.object_key \
             FROM commerce.product_meta_media_assets AS link \
             INNER JOIN commerce.media_assets AS media \
                ON media.store_id=link.store_id AND media.id=link.media_asset_id \
             WHERE link.store_id=$1 AND link.product_id=$2 \
             ORDER BY link.meta_path, media.id",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(product_meta_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub(crate) async fn attach_product(
        &self,
        actor: AdminActor,
        record: ProductMediaAssetLinkRecord,
        expected_revision: Option<i64>,
    ) -> Result<(i64, ProductMediaAssetItem), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        ensure_product(&mut tx, record.store_id, record.product_id).await?;
        ensure_product_revision(
            &mut tx,
            record.store_id,
            record.product_id,
            expected_revision,
        )
        .await?;
        ensure_ready_asset(&mut tx, record.store_id, record.media_asset_id, None).await?;
        sqlx::query(
            "INSERT INTO commerce.product_media_assets \
             (store_id, product_id, media_asset_id, alt_text, position) \
             VALUES ($1,$2,$3,$4,$5) \
             ON CONFLICT (store_id, product_id, media_asset_id) DO UPDATE \
                 SET alt_text=EXCLUDED.alt_text, position=EXCLUDED.position, archived_at=NULL",
        )
        .bind(record.store_id.as_uuid())
        .bind(record.product_id.as_uuid())
        .bind(record.media_asset_id.as_uuid())
        .bind(&record.alt_text)
        .bind(i16::try_from(record.position).map_err(|_| invalid_snapshot())?)
        .execute(&mut *tx)
        .await
        .map_err(map_product_media_error)?;
        let row = load_product(
            &mut tx,
            record.store_id,
            record.product_id,
            record.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(record.media_asset_id))?;
        let revision = touch_product(
            &mut tx,
            record.store_id,
            record.product_id,
            record.changed_at,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        Ok((revision, product_item(row)?))
    }

    pub(crate) async fn attach_product_option_value(
        &self,
        actor: AdminActor,
        record: ProductOptionValueMediaAssetLinkRecord,
        expected_revision: Option<i64>,
    ) -> Result<(i64, ProductMediaAssetItem), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        ensure_product(&mut tx, record.store_id, record.product_id).await?;
        ensure_product_revision(
            &mut tx,
            record.store_id,
            record.product_id,
            expected_revision,
        )
        .await?;
        ensure_product_option_value(
            &mut tx,
            record.store_id,
            record.product_id,
            record.option_id,
            record.option_value_id,
            true,
        )
        .await?;
        ensure_ready_asset(&mut tx, record.store_id, record.media_asset_id, None).await?;
        sqlx::query(
            "INSERT INTO commerce.product_option_value_media_assets \
             (store_id, product_id, option_id, option_value_id, media_asset_id, alt_text, position) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             ON CONFLICT (store_id, product_id, option_id, option_value_id, media_asset_id) DO UPDATE \
                 SET alt_text=EXCLUDED.alt_text, position=EXCLUDED.position, archived_at=NULL",
        )
        .bind(record.store_id.as_uuid())
        .bind(record.product_id.as_uuid())
        .bind(record.option_id.as_uuid())
        .bind(record.option_value_id.as_uuid())
        .bind(record.media_asset_id.as_uuid())
        .bind(&record.alt_text)
        .bind(i16::try_from(record.position).map_err(|_| invalid_snapshot())?)
        .execute(&mut *tx)
        .await
        .map_err(map_product_option_value_media_error)?;
        let row = load_product_option_value(
            &mut tx,
            record.store_id,
            record.product_id,
            record.option_id,
            record.option_value_id,
            record.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(record.media_asset_id))?;
        let revision = touch_product(
            &mut tx,
            record.store_id,
            record.product_id,
            record.changed_at,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        Ok((revision, product_item(row)?))
    }

    pub(crate) async fn attach_product_variant(
        &self,
        actor: AdminActor,
        record: ProductVariantMediaAssetLinkRecord,
        expected_revision: Option<i64>,
    ) -> Result<(i64, ProductMediaAssetItem), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        ensure_product(&mut tx, record.store_id, record.product_id).await?;
        ensure_product_revision(
            &mut tx,
            record.store_id,
            record.product_id,
            expected_revision,
        )
        .await?;
        ensure_product_variant(
            &mut tx,
            record.store_id,
            record.product_id,
            record.product_variant_id,
            true,
        )
        .await?;
        ensure_ready_asset(&mut tx, record.store_id, record.media_asset_id, None).await?;
        sqlx::query(
            "INSERT INTO commerce.product_variant_media_assets \
             (store_id, product_id, product_variant_id, media_asset_id, alt_text, position) \
             VALUES ($1,$2,$3,$4,$5,$6) \
             ON CONFLICT (store_id, product_id, product_variant_id, media_asset_id) DO UPDATE \
                 SET alt_text=EXCLUDED.alt_text, position=EXCLUDED.position, archived_at=NULL",
        )
        .bind(record.store_id.as_uuid())
        .bind(record.product_id.as_uuid())
        .bind(record.product_variant_id.as_uuid())
        .bind(record.media_asset_id.as_uuid())
        .bind(&record.alt_text)
        .bind(i16::try_from(record.position).map_err(|_| invalid_snapshot())?)
        .execute(&mut *tx)
        .await
        .map_err(map_product_variant_media_error)?;
        let row = load_product_variant(
            &mut tx,
            record.store_id,
            record.product_id,
            record.product_variant_id,
            record.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(record.media_asset_id))?;
        let revision = touch_product(
            &mut tx,
            record.store_id,
            record.product_id,
            record.changed_at,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        Ok((revision, product_item(row)?))
    }

    pub(crate) async fn replace_product(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        items: Vec<crate::catalog::ProductMediaItemInput>,
        changed_at: OffsetDateTime,
        expected_revision: Option<i64>,
    ) -> Result<(i64, Vec<ProductMediaAssetItem>), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        ensure_product(&mut tx, store_id, product_id).await?;
        ensure_product_revision(&mut tx, store_id, product_id, expected_revision).await?;
        let previous = active_product_media_asset_ids(&mut tx, store_id, product_id).await?;
        archive_product_links(&mut tx, store_id, product_id, changed_at).await?;
        for item in &items {
            ensure_ready_asset(&mut tx, store_id, item.media_asset_id, None).await?;
            insert_product_link(&mut tx, store_id, product_id, item).await?;
        }
        for media_asset_id in previous {
            archive_unreferenced_asset(&mut tx, store_id, media_asset_id, changed_at).await?;
        }
        let revision = touch_product(&mut tx, store_id, product_id, changed_at).await?;
        let rows = product_media_rows(&mut tx, store_id, product_id)
            .await?
            .into_iter()
            .filter(|row| row.link_archived_at.is_none() && row.scope == "product")
            .collect::<Vec<_>>();
        tx.commit().await.map_err(database_error)?;
        Ok((
            revision,
            rows.into_iter()
                .map(product_item)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn replace_product_option_value(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        option_id: ProductOptionId,
        option_value_id: ProductOptionValueId,
        items: Vec<crate::catalog::ProductMediaItemInput>,
        changed_at: OffsetDateTime,
        expected_revision: Option<i64>,
    ) -> Result<(i64, Vec<ProductMediaAssetItem>), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        ensure_product(&mut tx, store_id, product_id).await?;
        ensure_product_revision(&mut tx, store_id, product_id, expected_revision).await?;
        ensure_product_option_value(
            &mut tx,
            store_id,
            product_id,
            option_id,
            option_value_id,
            true,
        )
        .await?;
        let previous = active_option_value_media_asset_ids(
            &mut tx,
            store_id,
            product_id,
            option_id,
            option_value_id,
        )
        .await?;
        archive_option_value_links(
            &mut tx,
            store_id,
            product_id,
            option_id,
            option_value_id,
            changed_at,
        )
        .await?;
        for item in &items {
            ensure_ready_asset(&mut tx, store_id, item.media_asset_id, None).await?;
            insert_option_value_link(
                &mut tx,
                store_id,
                product_id,
                option_id,
                option_value_id,
                item,
            )
            .await?;
        }
        for media_asset_id in previous {
            archive_unreferenced_asset(&mut tx, store_id, media_asset_id, changed_at).await?;
        }
        let revision = touch_product(&mut tx, store_id, product_id, changed_at).await?;
        let rows = product_media_rows(&mut tx, store_id, product_id)
            .await?
            .into_iter()
            .filter(|row| {
                row.link_archived_at.is_none()
                    && row.option_id == Some(option_id.as_uuid())
                    && row.option_value_id == Some(option_value_id.as_uuid())
            })
            .collect::<Vec<_>>();
        tx.commit().await.map_err(database_error)?;
        Ok((
            revision,
            rows.into_iter()
                .map(product_item)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn replace_product_variant(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        product_variant_id: ProductVariantId,
        items: Vec<crate::catalog::ProductMediaItemInput>,
        changed_at: OffsetDateTime,
        expected_revision: Option<i64>,
    ) -> Result<(i64, Vec<ProductMediaAssetItem>), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        ensure_product(&mut tx, store_id, product_id).await?;
        ensure_product_revision(&mut tx, store_id, product_id, expected_revision).await?;
        ensure_product_variant(&mut tx, store_id, product_id, product_variant_id, true).await?;
        let previous =
            active_variant_media_asset_ids(&mut tx, store_id, product_id, product_variant_id)
                .await?;
        archive_variant_links(
            &mut tx,
            store_id,
            product_id,
            product_variant_id,
            changed_at,
        )
        .await?;
        for item in &items {
            ensure_ready_asset(&mut tx, store_id, item.media_asset_id, None).await?;
            insert_variant_link(&mut tx, store_id, product_id, product_variant_id, item).await?;
        }
        for media_asset_id in previous {
            archive_unreferenced_asset(&mut tx, store_id, media_asset_id, changed_at).await?;
        }
        let revision = touch_product(&mut tx, store_id, product_id, changed_at).await?;
        let rows = product_media_rows(&mut tx, store_id, product_id)
            .await?
            .into_iter()
            .filter(|row| {
                row.link_archived_at.is_none()
                    && row.product_variant_id == Some(product_variant_id.as_uuid())
            })
            .collect::<Vec<_>>();
        tx.commit().await.map_err(database_error)?;
        Ok((
            revision,
            rows.into_iter()
                .map(product_item)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    pub(crate) async fn batch_replace_product(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        targets: Vec<crate::catalog::BatchReplaceProductMediaTarget>,
        expected_revision: Option<i64>,
        changed_at: OffsetDateTime,
    ) -> Result<(i64, Vec<ProductMediaAssetItem>), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let (status, current_revision) = sqlx::query_as::<_, (String, i64)>(
            "SELECT status::text, revision FROM commerce.products \
             WHERE store_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| not_found_product(product_id))?;
        if status == "archived" {
            return Err(not_found_product(product_id));
        }
        if expected_revision.is_some_and(|expected| expected != current_revision) {
            return Err(ApplicationError::Conflict {
                code: "product_revision_mismatch",
                message: "the Product changed; refresh the workspace and retry with its current revision",
            });
        }

        let mut previous_assets = HashSet::new();
        for target in &targets {
            match target.target {
                crate::catalog::ProductMediaTarget::Product => {
                    previous_assets.extend(
                        active_product_media_asset_ids(&mut tx, store_id, product_id).await?,
                    );
                    archive_product_links(&mut tx, store_id, product_id, changed_at).await?;
                    for item in &target.items {
                        ensure_ready_asset(&mut tx, store_id, item.media_asset_id, None).await?;
                        insert_product_link(&mut tx, store_id, product_id, item).await?;
                    }
                }
                crate::catalog::ProductMediaTarget::OptionValue {
                    option_id,
                    option_value_id,
                } => {
                    ensure_product_option_value(
                        &mut tx,
                        store_id,
                        product_id,
                        option_id,
                        option_value_id,
                        true,
                    )
                    .await?;
                    previous_assets.extend(
                        active_option_value_media_asset_ids(
                            &mut tx,
                            store_id,
                            product_id,
                            option_id,
                            option_value_id,
                        )
                        .await?,
                    );
                    archive_option_value_links(
                        &mut tx,
                        store_id,
                        product_id,
                        option_id,
                        option_value_id,
                        changed_at,
                    )
                    .await?;
                    for item in &target.items {
                        ensure_ready_asset(&mut tx, store_id, item.media_asset_id, None).await?;
                        insert_option_value_link(
                            &mut tx,
                            store_id,
                            product_id,
                            option_id,
                            option_value_id,
                            item,
                        )
                        .await?;
                    }
                }
                crate::catalog::ProductMediaTarget::Variant { product_variant_id } => {
                    ensure_product_variant(&mut tx, store_id, product_id, product_variant_id, true)
                        .await?;
                    previous_assets.extend(
                        active_variant_media_asset_ids(
                            &mut tx,
                            store_id,
                            product_id,
                            product_variant_id,
                        )
                        .await?,
                    );
                    archive_variant_links(
                        &mut tx,
                        store_id,
                        product_id,
                        product_variant_id,
                        changed_at,
                    )
                    .await?;
                    for item in &target.items {
                        ensure_ready_asset(&mut tx, store_id, item.media_asset_id, None).await?;
                        insert_variant_link(
                            &mut tx,
                            store_id,
                            product_id,
                            product_variant_id,
                            item,
                        )
                        .await?;
                    }
                }
            }
        }
        for media_asset_id in previous_assets {
            archive_unreferenced_asset(&mut tx, store_id, media_asset_id, changed_at).await?;
        }
        let revision = touch_product(&mut tx, store_id, product_id, changed_at).await?;
        let target_set = targets
            .iter()
            .map(|target| target.target)
            .collect::<HashSet<_>>();
        let rows = product_media_rows(&mut tx, store_id, product_id)
            .await?
            .into_iter()
            .filter(|row| {
                row.link_archived_at.is_none()
                    && target_set
                        .iter()
                        .any(|target| product_media_row_matches_target(row, *target))
            })
            .collect::<Vec<_>>();
        tx.commit().await.map_err(database_error)?;
        Ok((
            revision,
            rows.into_iter()
                .map(product_item)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    pub(crate) async fn attach_review(
        &self,
        actor: AdminActor,
        record: ReviewMediaAssetLinkRecord,
    ) -> Result<ReviewMediaAssetItem, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        ensure_review_can_receive_media(&mut tx, record.review_id, record.store_id).await?;
        ensure_ready_asset(
            &mut tx,
            record.store_id,
            record.media_asset_id,
            Some(MediaKind::Image),
        )
        .await?;
        sqlx::query(
            "INSERT INTO commerce.review_media_assets \
             (store_id, review_id, media_asset_id, alt_text, position) \
             VALUES ($1,$2,$3,$4,$5) \
             ON CONFLICT (store_id, review_id, media_asset_id) DO UPDATE \
                 SET alt_text=EXCLUDED.alt_text, position=EXCLUDED.position, archived_at=NULL",
        )
        .bind(record.store_id.as_uuid())
        .bind(record.review_id.as_uuid())
        .bind(record.media_asset_id.as_uuid())
        .bind(&record.alt_text)
        .bind(i16::try_from(record.position).map_err(|_| invalid_snapshot())?)
        .execute(&mut *tx)
        .await
        .map_err(map_review_media_error)?;
        let row = load_review(
            &mut tx,
            record.store_id,
            record.review_id,
            record.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(record.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        review_item(row)
    }

    pub(crate) async fn attach_product_meta(
        &self,
        actor: AdminActor,
        record: ProductMetaMediaAssetLinkRecord,
    ) -> Result<(i64, ProductMetaMediaAssetItem), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        ensure_product(&mut tx, record.store_id, record.product_id).await?;
        ensure_product_revision(
            &mut tx,
            record.store_id,
            record.product_id,
            record.expected_revision,
        )
        .await?;
        let segments = parse_json_pointer(&record.meta_path)?;
        let (metadata,) = sqlx::query_as::<_, (Option<Value>,)>(
            "SELECT meta FROM commerce.products \
             WHERE store_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(record.store_id.as_uuid())
        .bind(record.product_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| not_found_product(record.product_id))?;
        ensure_ready_asset(
            &mut tx,
            record.store_id,
            record.media_asset_id,
            Some(MediaKind::Image),
        )
        .await?;

        let previous_asset_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT media_asset_id FROM commerce.product_meta_media_assets \
             WHERE store_id=$1 AND product_id=$2 AND meta_path=$3 AND archived_at IS NULL \
             FOR UPDATE",
        )
        .bind(record.store_id.as_uuid())
        .bind(record.product_id.as_uuid())
        .bind(&record.meta_path)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;

        let mut metadata = metadata.unwrap_or_else(|| json!({}));
        set_media_reference(
            &mut metadata,
            &segments,
            record.media_asset_id.as_uuid(),
            &record.alt_text,
        )?;
        let revision = sqlx::query_scalar::<_, i64>(
            "UPDATE commerce.products SET meta=$3::jsonb, revision=revision+1, updated_at=$4 \
             WHERE store_id=$1 AND id=$2 RETURNING revision",
        )
        .bind(record.store_id.as_uuid())
        .bind(record.product_id.as_uuid())
        .bind(&metadata)
        .bind(record.changed_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;

        if previous_asset_id.is_some_and(|id| id != record.media_asset_id.as_uuid()) {
            sqlx::query(
                "UPDATE commerce.product_meta_media_assets SET archived_at=$4 \
                 WHERE store_id=$1 AND product_id=$2 AND meta_path=$3 AND archived_at IS NULL",
            )
            .bind(record.store_id.as_uuid())
            .bind(record.product_id.as_uuid())
            .bind(&record.meta_path)
            .bind(record.changed_at)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        }
        sqlx::query(
            "INSERT INTO commerce.product_meta_media_assets \
             (store_id, product_id, media_asset_id, meta_path, alt_text) \
             VALUES ($1,$2,$3,$4,$5) \
             ON CONFLICT (store_id, product_id, meta_path, media_asset_id) DO UPDATE \
                 SET alt_text=EXCLUDED.alt_text, archived_at=NULL",
        )
        .bind(record.store_id.as_uuid())
        .bind(record.product_id.as_uuid())
        .bind(record.media_asset_id.as_uuid())
        .bind(&record.meta_path)
        .bind(&record.alt_text)
        .execute(&mut *tx)
        .await
        .map_err(map_product_meta_media_error)?;
        if let Some(previous_asset_id) =
            previous_asset_id.filter(|id| *id != record.media_asset_id.as_uuid())
        {
            archive_unreferenced_asset(
                &mut tx,
                record.store_id,
                MediaAssetId::from_uuid(previous_asset_id),
                record.changed_at,
            )
            .await?;
        }
        let row = load_product_meta(
            &mut tx,
            record.store_id,
            record.product_id,
            &record.meta_path,
            record.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(record.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        Ok((revision, product_meta_item(row)?))
    }

    pub(crate) async fn mark_ready(
        &self,
        actor: AdminActor,
        mutation: MediaAssetMutation,
        public_url: &str,
    ) -> Result<MediaAssetItem, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let changed = sqlx::query(
            "UPDATE commerce.media_assets \
             SET status='ready', public_url=$3, ready_at=$4, updated_at=$4 \
             WHERE store_id=$1 AND id=$2 AND status='pending'",
        )
        .bind(mutation.store_id.as_uuid())
        .bind(mutation.media_asset_id.as_uuid())
        .bind(public_url)
        .bind(mutation.changed_at)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?
        .rows_affected();
        let row = load_asset(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        if changed == 0 && row.status != "ready" {
            return Err(ApplicationError::Conflict {
                code: "media_asset_not_pending",
                message: "the Media Asset is not pending upload",
            });
        }
        tx.commit().await.map_err(database_error)?;
        item(row)
    }

    pub(crate) async fn archive_asset(
        &self,
        actor: AdminActor,
        mutation: MediaAssetMutation,
    ) -> Result<MediaAssetItem, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let row = load_asset_for_update(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        if row.status != "archived" {
            let in_use: bool = sqlx::query_scalar(
                "SELECT EXISTS ( \
                    SELECT 1 FROM commerce.product_media_assets \
                    WHERE store_id=$1 AND media_asset_id=$2 AND archived_at IS NULL \
                    UNION ALL \
                    SELECT 1 FROM commerce.product_option_value_media_assets \
                    WHERE store_id=$1 AND media_asset_id=$2 AND archived_at IS NULL \
                    UNION ALL \
                    SELECT 1 FROM commerce.product_variant_media_assets \
                    WHERE store_id=$1 AND media_asset_id=$2 AND archived_at IS NULL \
                    UNION ALL \
                    SELECT 1 FROM commerce.review_media_assets \
                    WHERE store_id=$1 AND media_asset_id=$2 AND archived_at IS NULL \
                    UNION ALL \
                    SELECT 1 FROM commerce.product_meta_media_assets \
                    WHERE store_id=$1 AND media_asset_id=$2 AND archived_at IS NULL \
                )",
            )
            .bind(mutation.store_id.as_uuid())
            .bind(mutation.media_asset_id.as_uuid())
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
            if in_use {
                return Err(ApplicationError::Conflict {
                    code: "media_asset_in_use",
                    message: "detach the Media Asset from every target before archiving it",
                });
            }
            sqlx::query(
                "UPDATE commerce.media_assets SET status='archived', archived_at=$3, updated_at=$3 \
                 WHERE store_id=$1 AND id=$2 AND status<>'archived'",
            )
            .bind(mutation.store_id.as_uuid())
            .bind(mutation.media_asset_id.as_uuid())
            .bind(mutation.changed_at)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        }
        let row = load_asset(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        item(row)
    }

    pub(crate) async fn restore_asset(
        &self,
        actor: AdminActor,
        mutation: MediaAssetMutation,
    ) -> Result<MediaAssetItem, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let row = load_asset_for_update(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        if row.status == MediaAssetStatus::Ready.as_str() {
            tx.commit().await.map_err(database_error)?;
            return item(row);
        }
        if row.status != MediaAssetStatus::Archived.as_str() {
            return Err(ApplicationError::Conflict {
                code: "media_asset_not_archived",
                message: "only an archived Media Asset can be restored",
            });
        }
        if row.public_url.is_none() {
            return Err(ApplicationError::Conflict {
                code: "media_asset_object_unavailable",
                message: "this archived Media Asset has no retained object URL and cannot be restored",
            });
        }
        sqlx::query(
            "UPDATE commerce.media_assets \
             SET status='ready', archived_at=NULL, updated_at=$3 \
             WHERE store_id=$1 AND id=$2",
        )
        .bind(mutation.store_id.as_uuid())
        .bind(mutation.media_asset_id.as_uuid())
        .bind(mutation.changed_at)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        let row = load_asset(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        item(row)
    }

    pub(crate) async fn archive_product(
        &self,
        actor: AdminActor,
        mutation: ProductMediaAssetMutation,
    ) -> Result<(i64, ProductMediaAssetItem), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        lock_product(&mut tx, mutation.store_id, mutation.product_id).await?;
        ensure_product_revision(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.expected_revision,
        )
        .await?;
        load_asset_for_update(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        load_product(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        sqlx::query(
            "UPDATE commerce.product_media_assets \
             SET archived_at=COALESCE(archived_at,$4) \
             WHERE store_id=$1 AND product_id=$2 AND media_asset_id=$3",
        )
        .bind(mutation.store_id.as_uuid())
        .bind(mutation.product_id.as_uuid())
        .bind(mutation.media_asset_id.as_uuid())
        .bind(mutation.changed_at)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        archive_unreferenced_asset(
            &mut tx,
            mutation.store_id,
            mutation.media_asset_id,
            mutation.changed_at,
        )
        .await?;
        let revision = touch_product(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.changed_at,
        )
        .await?;
        let row = load_product(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        Ok((revision, product_item(row)?))
    }

    pub(crate) async fn archive_product_option_value(
        &self,
        actor: AdminActor,
        mutation: ProductOptionValueMediaAssetMutation,
    ) -> Result<(i64, ProductMediaAssetItem), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        lock_product(&mut tx, mutation.store_id, mutation.product_id).await?;
        ensure_product_revision(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.expected_revision,
        )
        .await?;
        ensure_product_option_value(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.option_id,
            mutation.option_value_id,
            true,
        )
        .await?;
        load_asset_for_update(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        load_product_option_value(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.option_id,
            mutation.option_value_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        sqlx::query(
            "UPDATE commerce.product_option_value_media_assets \
             SET archived_at=COALESCE(archived_at,$5) \
             WHERE store_id=$1 AND product_id=$2 AND option_id=$3 AND option_value_id=$4 \
               AND media_asset_id=$6",
        )
        .bind(mutation.store_id.as_uuid())
        .bind(mutation.product_id.as_uuid())
        .bind(mutation.option_id.as_uuid())
        .bind(mutation.option_value_id.as_uuid())
        .bind(mutation.changed_at)
        .bind(mutation.media_asset_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        archive_unreferenced_asset(
            &mut tx,
            mutation.store_id,
            mutation.media_asset_id,
            mutation.changed_at,
        )
        .await?;
        let revision = touch_product(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.changed_at,
        )
        .await?;
        let row = load_product_option_value(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.option_id,
            mutation.option_value_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        Ok((revision, product_item(row)?))
    }

    pub(crate) async fn archive_product_variant(
        &self,
        actor: AdminActor,
        mutation: ProductVariantMediaAssetMutation,
    ) -> Result<(i64, ProductMediaAssetItem), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        lock_product(&mut tx, mutation.store_id, mutation.product_id).await?;
        ensure_product_revision(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.expected_revision,
        )
        .await?;
        ensure_product_variant(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.product_variant_id,
            true,
        )
        .await?;
        load_asset_for_update(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        load_product_variant(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.product_variant_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        sqlx::query(
            "UPDATE commerce.product_variant_media_assets \
             SET archived_at=COALESCE(archived_at,$4) \
             WHERE store_id=$1 AND product_id=$2 AND product_variant_id=$3 \
               AND media_asset_id=$5",
        )
        .bind(mutation.store_id.as_uuid())
        .bind(mutation.product_id.as_uuid())
        .bind(mutation.product_variant_id.as_uuid())
        .bind(mutation.changed_at)
        .bind(mutation.media_asset_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        archive_unreferenced_asset(
            &mut tx,
            mutation.store_id,
            mutation.media_asset_id,
            mutation.changed_at,
        )
        .await?;
        let revision = touch_product(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.changed_at,
        )
        .await?;
        let row = load_product_variant(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.product_variant_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        Ok((revision, product_item(row)?))
    }

    pub(crate) async fn archive_review(
        &self,
        actor: AdminActor,
        mutation: ReviewMediaAssetMutation,
    ) -> Result<ReviewMediaAssetItem, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        lock_review(&mut tx, mutation.store_id, mutation.review_id).await?;
        load_asset_for_update(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        load_review(
            &mut tx,
            mutation.store_id,
            mutation.review_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        sqlx::query(
            "UPDATE commerce.review_media_assets \
             SET archived_at=COALESCE(archived_at,$3) \
             WHERE store_id=$1 AND review_id=$2 AND media_asset_id=$4",
        )
        .bind(mutation.store_id.as_uuid())
        .bind(mutation.review_id.as_uuid())
        .bind(mutation.changed_at)
        .bind(mutation.media_asset_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        archive_unreferenced_asset(
            &mut tx,
            mutation.store_id,
            mutation.media_asset_id,
            mutation.changed_at,
        )
        .await?;
        let row = load_review(
            &mut tx,
            mutation.store_id,
            mutation.review_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        review_item(row)
    }

    pub(crate) async fn archive_product_meta(
        &self,
        actor: AdminActor,
        mutation: ProductMetaMediaAssetMutation,
    ) -> Result<(i64, ProductMetaMediaAssetItem), ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let segments = parse_json_pointer(&mutation.meta_path)?;
        let (metadata, current_revision) = sqlx::query_as::<_, (Option<Value>, i64)>(
            "SELECT meta, revision FROM commerce.products \
             WHERE store_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(mutation.store_id.as_uuid())
        .bind(mutation.product_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or_else(|| not_found_product(mutation.product_id))?;
        ensure_product_revision(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            mutation.expected_revision,
        )
        .await?;
        load_asset_for_update(&mut tx, mutation.store_id, mutation.media_asset_id)
            .await?
            .ok_or_else(|| not_found(mutation.media_asset_id))?;
        load_product_meta(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            &mutation.meta_path,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        let archived_link = sqlx::query(
            "UPDATE commerce.product_meta_media_assets \
             SET archived_at=COALESCE(archived_at,$5) \
             WHERE store_id=$1 AND product_id=$2 AND meta_path=$3 AND media_asset_id=$4 \
               AND archived_at IS NULL",
        )
        .bind(mutation.store_id.as_uuid())
        .bind(mutation.product_id.as_uuid())
        .bind(&mutation.meta_path)
        .bind(mutation.media_asset_id.as_uuid())
        .bind(mutation.changed_at)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?
        .rows_affected()
            > 0;

        let mut metadata = metadata.unwrap_or_else(|| json!({}));
        let revision = if pointer_media_asset_id(&metadata, &segments)
            == Some(mutation.media_asset_id.as_uuid())
            && clear_media_reference(&mut metadata, &segments)
        {
            sqlx::query_scalar::<_, i64>(
                "UPDATE commerce.products SET meta=$3::jsonb, revision=revision+1, updated_at=$4 \
                 WHERE store_id=$1 AND id=$2 RETURNING revision",
            )
            .bind(mutation.store_id.as_uuid())
            .bind(mutation.product_id.as_uuid())
            .bind(&metadata)
            .bind(mutation.changed_at)
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?
        } else {
            if archived_link {
                touch_product(
                    &mut tx,
                    mutation.store_id,
                    mutation.product_id,
                    mutation.changed_at,
                )
                .await?
            } else {
                current_revision
            }
        };
        archive_unreferenced_asset(
            &mut tx,
            mutation.store_id,
            mutation.media_asset_id,
            mutation.changed_at,
        )
        .await?;
        let row = load_product_meta(
            &mut tx,
            mutation.store_id,
            mutation.product_id,
            &mutation.meta_path,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        Ok((revision, product_meta_item(row)?))
    }
}

async fn product_media_rows(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
) -> Result<Vec<ProductMediaRow>, ApplicationError> {
    sqlx::query_as::<_, ProductMediaRow>(
        "SELECT media.id AS asset_id, media.store_id, link.product_id, \
                'product'::text AS scope, NULL::uuid AS option_id, \
                NULL::uuid AS option_value_id, NULL::uuid AS product_variant_id, \
                media.file_name, media.media_type, media.media_kind::text, \
                media.byte_size, media.sha256_digest, media.status::text, \
                media.public_url, media.created_at, media.updated_at, link.alt_text, \
                link.position, link.archived_at AS link_archived_at, media.object_key \
         FROM commerce.product_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         WHERE link.store_id=$1 AND link.product_id=$2 \
         UNION ALL \
         SELECT media.id AS asset_id, media.store_id, link.product_id, \
                'option_value'::text AS scope, link.option_id, link.option_value_id, \
                NULL::uuid AS product_variant_id, media.file_name, media.media_type, \
                media.media_kind::text, media.byte_size, media.sha256_digest, \
                media.status::text, media.public_url, media.created_at, media.updated_at, \
                link.alt_text, link.position, link.archived_at AS link_archived_at, \
                media.object_key \
         FROM commerce.product_option_value_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         WHERE link.store_id=$1 AND link.product_id=$2 \
         UNION ALL \
         SELECT media.id AS asset_id, media.store_id, link.product_id, \
                'variant'::text AS scope, NULL::uuid AS option_id, \
                NULL::uuid AS option_value_id, link.product_variant_id, \
                media.file_name, media.media_type, media.media_kind::text, \
                media.byte_size, media.sha256_digest, media.status::text, \
                media.public_url, media.created_at, media.updated_at, link.alt_text, \
                link.position, link.archived_at AS link_archived_at, media.object_key \
         FROM commerce.product_variant_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         WHERE link.store_id=$1 AND link.product_id=$2 \
         ORDER BY position, scope, asset_id",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)
}

fn product_media_row_matches_target(
    row: &ProductMediaRow,
    target: crate::catalog::ProductMediaTarget,
) -> bool {
    match target {
        crate::catalog::ProductMediaTarget::Product => row.scope == "product",
        crate::catalog::ProductMediaTarget::OptionValue {
            option_id,
            option_value_id,
        } => {
            row.scope == "option_value"
                && row.option_id == Some(option_id.as_uuid())
                && row.option_value_id == Some(option_value_id.as_uuid())
        }
        crate::catalog::ProductMediaTarget::Variant { product_variant_id } => {
            row.scope == "variant" && row.product_variant_id == Some(product_variant_id.as_uuid())
        }
    }
}

async fn insert_product_link(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    item: &crate::catalog::ProductMediaItemInput,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.product_media_assets \
         (store_id, product_id, media_asset_id, alt_text, position) \
         VALUES ($1,$2,$3,$4,$5) \
         ON CONFLICT (store_id, product_id, media_asset_id) DO UPDATE \
             SET alt_text=EXCLUDED.alt_text, position=EXCLUDED.position, archived_at=NULL",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(item.media_asset_id.as_uuid())
    .bind(&item.alt_text)
    .bind(i16::try_from(item.position).map_err(|_| invalid_snapshot())?)
    .execute(&mut **tx)
    .await
    .map_err(map_product_media_error)?;
    Ok(())
}

async fn insert_option_value_link(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    option_id: ProductOptionId,
    option_value_id: ProductOptionValueId,
    item: &crate::catalog::ProductMediaItemInput,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.product_option_value_media_assets \
         (store_id, product_id, option_id, option_value_id, media_asset_id, alt_text, position) \
         VALUES ($1,$2,$3,$4,$5,$6,$7) \
         ON CONFLICT (store_id, product_id, option_id, option_value_id, media_asset_id) DO UPDATE \
             SET alt_text=EXCLUDED.alt_text, position=EXCLUDED.position, archived_at=NULL",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(option_id.as_uuid())
    .bind(option_value_id.as_uuid())
    .bind(item.media_asset_id.as_uuid())
    .bind(&item.alt_text)
    .bind(i16::try_from(item.position).map_err(|_| invalid_snapshot())?)
    .execute(&mut **tx)
    .await
    .map_err(map_product_option_value_media_error)?;
    Ok(())
}

async fn insert_variant_link(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    product_variant_id: ProductVariantId,
    item: &crate::catalog::ProductMediaItemInput,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO commerce.product_variant_media_assets \
         (store_id, product_id, product_variant_id, media_asset_id, alt_text, position) \
         VALUES ($1,$2,$3,$4,$5,$6) \
         ON CONFLICT (store_id, product_id, product_variant_id, media_asset_id) DO UPDATE \
             SET alt_text=EXCLUDED.alt_text, position=EXCLUDED.position, archived_at=NULL",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(product_variant_id.as_uuid())
    .bind(item.media_asset_id.as_uuid())
    .bind(&item.alt_text)
    .bind(i16::try_from(item.position).map_err(|_| invalid_snapshot())?)
    .execute(&mut **tx)
    .await
    .map_err(map_product_variant_media_error)?;
    Ok(())
}

async fn active_product_media_asset_ids(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
) -> Result<Vec<MediaAssetId>, ApplicationError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT media_asset_id FROM commerce.product_media_assets \
         WHERE store_id=$1 AND product_id=$2 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map(|ids| ids.into_iter().map(MediaAssetId::from_uuid).collect())
    .map_err(database_error)
}

async fn active_option_value_media_asset_ids(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    option_id: ProductOptionId,
    option_value_id: ProductOptionValueId,
) -> Result<Vec<MediaAssetId>, ApplicationError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT media_asset_id FROM commerce.product_option_value_media_assets \
         WHERE store_id=$1 AND product_id=$2 AND option_id=$3 AND option_value_id=$4 \
           AND archived_at IS NULL FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(option_id.as_uuid())
    .bind(option_value_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map(|ids| ids.into_iter().map(MediaAssetId::from_uuid).collect())
    .map_err(database_error)
}

async fn active_variant_media_asset_ids(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    product_variant_id: ProductVariantId,
) -> Result<Vec<MediaAssetId>, ApplicationError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT media_asset_id FROM commerce.product_variant_media_assets \
         WHERE store_id=$1 AND product_id=$2 AND product_variant_id=$3 \
           AND archived_at IS NULL FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(product_variant_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map(|ids| ids.into_iter().map(MediaAssetId::from_uuid).collect())
    .map_err(database_error)
}

async fn archive_product_links(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE commerce.product_media_assets SET archived_at=$3 \
         WHERE store_id=$1 AND product_id=$2 AND archived_at IS NULL",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(changed_at)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn archive_option_value_links(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    option_id: ProductOptionId,
    option_value_id: ProductOptionValueId,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE commerce.product_option_value_media_assets SET archived_at=$5 \
         WHERE store_id=$1 AND product_id=$2 AND option_id=$3 AND option_value_id=$4 \
           AND archived_at IS NULL",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(option_id.as_uuid())
    .bind(option_value_id.as_uuid())
    .bind(changed_at)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn archive_variant_links(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    product_variant_id: ProductVariantId,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE commerce.product_variant_media_assets SET archived_at=$4 \
         WHERE store_id=$1 AND product_id=$2 AND product_variant_id=$3 \
           AND archived_at IS NULL",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(product_variant_id.as_uuid())
    .bind(changed_at)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn insert_asset(
    tx: &mut Transaction<'_, Postgres>,
    record: &CreateMediaAssetRecord,
) -> Result<(), ApplicationError> {
    let digest = decode_digest(record.descriptor.sha256_hex())?;
    sqlx::query(
        "INSERT INTO commerce.media_assets \
         (id, store_id, object_key, file_name, media_type, media_kind, byte_size, \
          sha256_digest, status, created_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6::commerce.media_kind,$7,$8::bytea,'pending',$9,$9)",
    )
    .bind(record.id.as_uuid())
    .bind(record.store_id.as_uuid())
    .bind(&record.object_key)
    .bind(record.descriptor.file_name())
    .bind(record.descriptor.media_type())
    .bind(record.descriptor.kind().as_str())
    .bind(i64::try_from(record.descriptor.byte_size()).map_err(|_| invalid_snapshot())?)
    .bind(digest.as_slice())
    .bind(record.created_at)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn ensure_product(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
) -> Result<(), ApplicationError> {
    let status = sqlx::query_scalar::<_, String>(
        "SELECT status::text FROM commerce.products \
         WHERE store_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    if status.as_deref() == Some("archived") || status.is_none() {
        return Err(not_found_product(product_id));
    }
    Ok(())
}

async fn ensure_product_revision(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    expected_revision: Option<i64>,
) -> Result<(), ApplicationError> {
    let Some(expected_revision) = expected_revision else {
        return Ok(());
    };
    let current_revision = sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM commerce.products WHERE store_id=$1 AND id=$2",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    .ok_or_else(|| not_found_product(product_id))?;
    if current_revision != expected_revision {
        return Err(ApplicationError::Conflict {
            code: "product_revision_mismatch",
            message: "the Product changed; refresh the workspace and retry with its current revision",
        });
    }
    Ok(())
}

async fn ensure_product_option_value(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    option_id: ProductOptionId,
    option_value_id: ProductOptionValueId,
    error_on_missing: bool,
) -> Result<bool, ApplicationError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 \
         FROM commerce.product_option_values AS value \
         INNER JOIN commerce.product_options AS option \
          ON option.store_id=value.store_id \
          AND option.product_id=value.product_id \
          AND option.id=value.option_id \
         WHERE value.store_id=$1 AND value.product_id=$2 AND value.option_id=$3 \
           AND value.id=$4 AND value.archived_at IS NULL AND option.archived_at IS NULL)",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(option_id.as_uuid())
    .bind(option_value_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    if !valid && error_on_missing {
        return Err(ApplicationError::Validation {
            violations: vec![FieldViolation {
                field: "option_value_id",
                reason: "must identify an Option Value of the same Product and Option".into(),
            }],
        });
    }
    Ok(valid)
}

async fn ensure_product_variant(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    product_variant_id: ProductVariantId,
    error_on_missing: bool,
) -> Result<bool, ApplicationError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM commerce.product_variants \
         WHERE store_id=$1 AND product_id=$2 AND id=$3 AND status='active')",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(product_variant_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    if !valid && error_on_missing {
        return Err(ApplicationError::Validation {
            violations: vec![FieldViolation {
                field: "product_variant_id",
                reason: "must identify a Variant of the same Product".into(),
            }],
        });
    }
    Ok(valid)
}

async fn lock_product(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
) -> Result<(), ApplicationError> {
    let exists =
        sqlx::query("SELECT 1 FROM commerce.products WHERE store_id=$1 AND id=$2 FOR UPDATE")
            .bind(store_id.as_uuid())
            .bind(product_id.as_uuid())
            .fetch_optional(&mut **tx)
            .await
            .map_err(database_error)?
            .is_some();
    if !exists {
        return Err(not_found_product(product_id));
    }
    Ok(())
}

async fn touch_product(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    changed_at: OffsetDateTime,
) -> Result<i64, ApplicationError> {
    sqlx::query_scalar::<_, i64>(
        "UPDATE commerce.products \
         SET revision=revision+1, updated_at=$3 \
         WHERE store_id=$1 AND id=$2 \
         RETURNING revision",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(changed_at)
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    .ok_or_else(|| not_found_product(product_id))
}

async fn lock_review(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    review_id: ReviewId,
) -> Result<(), ApplicationError> {
    let exists =
        sqlx::query("SELECT 1 FROM commerce.reviews WHERE store_id=$1 AND id=$2 FOR UPDATE")
            .bind(store_id.as_uuid())
            .bind(review_id.as_uuid())
            .fetch_optional(&mut **tx)
            .await
            .map_err(database_error)?
            .is_some();
    if !exists {
        return Err(not_found_review(review_id));
    }
    Ok(())
}

async fn ensure_review_can_receive_media(
    tx: &mut Transaction<'_, Postgres>,
    review_id: ReviewId,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    let row = sqlx::query_as::<_, (String, bool)>(
        "SELECT status::text, is_staff_reply FROM commerce.reviews \
         WHERE store_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(review_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    let Some((status, is_staff_reply)) = row else {
        return Err(ApplicationError::NotFound {
            resource: "review",
            id: review_id.as_uuid().to_string(),
        });
    };
    if is_staff_reply || status != "pending" {
        return Err(ApplicationError::Conflict {
            code: "review_media_not_uploadable",
            message: "review media can be attached only to a pending top-level review",
        });
    }
    Ok(())
}

async fn ensure_ready_asset(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    media_asset_id: MediaAssetId,
    expected_kind: Option<MediaKind>,
) -> Result<(), ApplicationError> {
    let row = load_asset_for_update(tx, store_id, media_asset_id)
        .await?
        .ok_or_else(|| not_found(media_asset_id))?;
    if row.status != MediaAssetStatus::Ready.as_str() {
        return Err(ApplicationError::Conflict {
            code: "media_asset_not_ready",
            message: "only a ready Media Asset can be attached",
        });
    }
    if expected_kind.is_some_and(|kind| row.media_kind != kind.as_str()) {
        return Err(ApplicationError::Validation {
            violations: vec![FieldViolation {
                field: "media_asset_id",
                reason: "the Media Asset kind is not valid for this attachment".into(),
            }],
        });
    }
    Ok(())
}

async fn archive_unreferenced_asset(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    media_asset_id: MediaAssetId,
    changed_at: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE commerce.media_assets AS media \
         SET status='archived', archived_at=$3, updated_at=$3 \
         WHERE media.store_id=$1 AND media.id=$2 AND media.status<>'archived' \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.product_media_assets AS product_link \
             WHERE product_link.store_id=media.store_id \
               AND product_link.media_asset_id=media.id \
               AND product_link.archived_at IS NULL \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.product_option_value_media_assets AS option_value_link \
             WHERE option_value_link.store_id=media.store_id \
               AND option_value_link.media_asset_id=media.id \
               AND option_value_link.archived_at IS NULL \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.product_variant_media_assets AS variant_link \
             WHERE variant_link.store_id=media.store_id \
               AND variant_link.media_asset_id=media.id \
               AND variant_link.archived_at IS NULL \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.review_media_assets AS review_link \
             WHERE review_link.store_id=media.store_id \
               AND review_link.media_asset_id=media.id \
               AND review_link.archived_at IS NULL \
           ) \
           AND NOT EXISTS ( \
             SELECT 1 FROM commerce.product_meta_media_assets AS meta_link \
             WHERE meta_link.store_id=media.store_id \
               AND meta_link.media_asset_id=media.id \
               AND meta_link.archived_at IS NULL \
           )",
    )
    .bind(store_id.as_uuid())
    .bind(media_asset_id.as_uuid())
    .bind(changed_at)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn load_asset(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    media_asset_id: MediaAssetId,
) -> Result<Option<MediaRow>, ApplicationError> {
    sqlx::query_as::<_, MediaRow>(
        "SELECT id, store_id, object_key, file_name, media_type, media_kind::text, \
                byte_size, sha256_digest, status::text, public_url, created_at, updated_at \
         FROM commerce.media_assets WHERE store_id=$1 AND id=$2",
    )
    .bind(store_id.as_uuid())
    .bind(media_asset_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)
}

async fn load_asset_for_update(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    media_asset_id: MediaAssetId,
) -> Result<Option<MediaRow>, ApplicationError> {
    sqlx::query_as::<_, MediaRow>(
        "SELECT id, store_id, object_key, file_name, media_type, media_kind::text, \
                byte_size, sha256_digest, status::text, public_url, created_at, updated_at \
         FROM commerce.media_assets WHERE store_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .bind(media_asset_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)
}

async fn load_product(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    media_asset_id: MediaAssetId,
) -> Result<Option<ProductMediaRow>, ApplicationError> {
    Ok(product_media_rows(tx, store_id, product_id)
        .await?
        .into_iter()
        .find(|row| row.asset_id == media_asset_id.as_uuid() && row.scope == "product"))
}

async fn load_product_option_value(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    option_id: ProductOptionId,
    option_value_id: ProductOptionValueId,
    media_asset_id: MediaAssetId,
) -> Result<Option<ProductMediaRow>, ApplicationError> {
    Ok(product_media_rows(tx, store_id, product_id)
        .await?
        .into_iter()
        .find(|row| {
            row.asset_id == media_asset_id.as_uuid()
                && row.option_id == Some(option_id.as_uuid())
                && row.option_value_id == Some(option_value_id.as_uuid())
        }))
}

async fn load_product_variant(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    product_variant_id: ProductVariantId,
    media_asset_id: MediaAssetId,
) -> Result<Option<ProductMediaRow>, ApplicationError> {
    Ok(product_media_rows(tx, store_id, product_id)
        .await?
        .into_iter()
        .find(|row| {
            row.asset_id == media_asset_id.as_uuid()
                && row.product_variant_id == Some(product_variant_id.as_uuid())
        }))
}

async fn load_review(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    review_id: ReviewId,
    media_asset_id: MediaAssetId,
) -> Result<Option<ReviewMediaRow>, ApplicationError> {
    sqlx::query_as::<_, ReviewMediaRow>(
        "SELECT media.id AS asset_id, media.store_id, link.review_id, media.file_name, \
                media.media_type, media.media_kind::text, media.byte_size, \
                media.sha256_digest, media.status::text, media.public_url, \
                media.created_at, media.updated_at, link.alt_text, link.position, \
                link.archived_at AS link_archived_at, media.object_key \
         FROM commerce.review_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         WHERE link.store_id=$1 AND link.review_id=$2 AND link.media_asset_id=$3",
    )
    .bind(store_id.as_uuid())
    .bind(review_id.as_uuid())
    .bind(media_asset_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)
}

async fn load_product_meta(
    tx: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
    meta_path: &str,
    media_asset_id: MediaAssetId,
) -> Result<Option<ProductMetaMediaRow>, ApplicationError> {
    sqlx::query_as::<_, ProductMetaMediaRow>(
        "SELECT media.id AS asset_id, media.store_id, link.product_id, link.meta_path, \
                media.file_name, media.media_type, media.media_kind::text, media.byte_size, \
                media.sha256_digest, media.status::text, media.public_url, \
                media.created_at, media.updated_at, link.alt_text, \
                link.archived_at AS link_archived_at, media.object_key \
         FROM commerce.product_meta_media_assets AS link \
         INNER JOIN commerce.media_assets AS media \
            ON media.store_id=link.store_id AND media.id=link.media_asset_id \
         WHERE link.store_id=$1 AND link.product_id=$2 AND link.meta_path=$3 \
           AND link.media_asset_id=$4",
    )
    .bind(store_id.as_uuid())
    .bind(product_id.as_uuid())
    .bind(meta_path)
    .bind(media_asset_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)
}

fn storage_record(row: MediaRow) -> Result<MediaAssetStorageRecord, ApplicationError> {
    let object_key = row.object_key.clone();
    Ok(MediaAssetStorageRecord {
        asset: media_item(row)?,
        object_key,
    })
}

fn item(row: MediaRow) -> Result<MediaAssetItem, ApplicationError> {
    media_item(row)
}

fn product_item(row: ProductMediaRow) -> Result<ProductMediaAssetItem, ApplicationError> {
    let scope = match row.scope.as_str() {
        "product" => ProductMediaScope::Product,
        "option_value" => ProductMediaScope::OptionValue {
            option_id: ProductOptionId::from_uuid(row.option_id.ok_or_else(invalid_snapshot)?),
            option_value_id: ProductOptionValueId::from_uuid(
                row.option_value_id.ok_or_else(invalid_snapshot)?,
            ),
        },
        "variant" => ProductMediaScope::Variant {
            product_variant_id: ProductVariantId::from_uuid(
                row.product_variant_id.ok_or_else(invalid_snapshot)?,
            ),
        },
        _ => return Err(invalid_snapshot()),
    };
    Ok(ProductMediaAssetItem {
        asset: media_item_from_product(&row)?,
        product_id: ProductId::from_uuid(row.product_id),
        scope,
        alt_text: row.alt_text,
        position: u16::try_from(row.position).map_err(|_| invalid_snapshot())?,
        archived_at: row.link_archived_at,
    })
}

fn review_item(row: ReviewMediaRow) -> Result<ReviewMediaAssetItem, ApplicationError> {
    Ok(ReviewMediaAssetItem {
        asset: media_item_from_review(&row)?,
        review_id: ReviewId::from_uuid(row.review_id),
        alt_text: row.alt_text,
        position: u16::try_from(row.position).map_err(|_| invalid_snapshot())?,
        archived_at: row.link_archived_at,
    })
}

fn product_meta_item(
    row: ProductMetaMediaRow,
) -> Result<ProductMetaMediaAssetItem, ApplicationError> {
    Ok(ProductMetaMediaAssetItem {
        asset: media_item_from_product_meta(&row)?,
        product_id: ProductId::from_uuid(row.product_id),
        meta_path: row.meta_path,
        alt_text: row.alt_text,
        archived_at: row.link_archived_at,
    })
}

fn media_item_from_product(row: &ProductMediaRow) -> Result<MediaAssetItem, ApplicationError> {
    media_item(MediaRow {
        id: row.asset_id,
        store_id: row.store_id,
        object_key: row.object_key.clone(),
        file_name: row.file_name.clone(),
        media_type: row.media_type.clone(),
        media_kind: row.media_kind.clone(),
        byte_size: row.byte_size,
        sha256_digest: row.sha256_digest.clone(),
        status: row.status.clone(),
        public_url: row.public_url.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn media_item_from_review(row: &ReviewMediaRow) -> Result<MediaAssetItem, ApplicationError> {
    media_item(MediaRow {
        id: row.asset_id,
        store_id: row.store_id,
        object_key: row.object_key.clone(),
        file_name: row.file_name.clone(),
        media_type: row.media_type.clone(),
        media_kind: row.media_kind.clone(),
        byte_size: row.byte_size,
        sha256_digest: row.sha256_digest.clone(),
        status: row.status.clone(),
        public_url: row.public_url.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn media_item_from_product_meta(
    row: &ProductMetaMediaRow,
) -> Result<MediaAssetItem, ApplicationError> {
    media_item(MediaRow {
        id: row.asset_id,
        store_id: row.store_id,
        object_key: row.object_key.clone(),
        file_name: row.file_name.clone(),
        media_type: row.media_type.clone(),
        media_kind: row.media_kind.clone(),
        byte_size: row.byte_size,
        sha256_digest: row.sha256_digest.clone(),
        status: row.status.clone(),
        public_url: row.public_url.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn media_item(row: MediaRow) -> Result<MediaAssetItem, ApplicationError> {
    let byte_size = u64::try_from(row.byte_size).map_err(|_| invalid_snapshot())?;
    let kind = match row.media_kind.as_str() {
        "image" => MediaKind::Image,
        "video" => MediaKind::Video,
        _ => return Err(invalid_snapshot()),
    };
    let status = MediaAssetStatus::parse(&row.status).ok_or_else(invalid_snapshot)?;
    Ok(MediaAssetItem {
        id: MediaAssetId::from_uuid(row.id),
        store_id: StoreId::from_uuid(row.store_id),
        file_name: row.file_name,
        media_type: row.media_type,
        kind,
        byte_size,
        sha256_hex: encode_digest(&row.sha256_digest)?,
        status,
        public_url: row.public_url,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn set_json_pointer(
    root: &mut Value,
    segments: &[String],
    replacement: Value,
) -> Result<(), ApplicationError> {
    if !root.is_object() {
        return Err(meta_path_violation(
            "the Product metadata root must be an object",
        ));
    }
    set_json_pointer_at(root, segments, replacement)
}

fn json_pointer_mut<'a>(root: &'a mut Value, segments: &[String]) -> Option<&'a mut Value> {
    let mut current = root;
    for segment in segments {
        current = match current {
            Value::Object(map) => map.get_mut(segment)?,
            Value::Array(array) => array.get_mut(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn set_media_reference(
    root: &mut Value,
    segments: &[String],
    media_asset_id: Uuid,
    alt_text: &str,
) -> Result<(), ApplicationError> {
    let replacement = json!({
        "media_asset_id": media_asset_id,
        "alt_text": alt_text,
    });
    if let Some(node) = json_pointer_mut(root, segments) {
        match node {
            Value::Object(map) => {
                map.remove("url");
                map.remove("media_type");
                map.insert("media_asset_id".into(), json!(media_asset_id));
                map.insert("alt_text".into(), json!(alt_text));
            }
            _ => *node = replacement,
        }
        return Ok(());
    }
    set_json_pointer(root, segments, replacement)
}

fn set_json_pointer_at(
    current: &mut Value,
    segments: &[String],
    replacement: Value,
) -> Result<(), ApplicationError> {
    let Some(segment) = segments.first() else {
        return Err(meta_path_violation(
            "the Media path must not point to the metadata root",
        ));
    };
    if segments.len() == 1 {
        match current {
            Value::Object(map) => {
                map.insert(segment.clone(), replacement);
                Ok(())
            }
            Value::Array(array) => {
                let index = array_index(segment)?;
                let Some(slot) = array.get_mut(index) else {
                    return Err(meta_path_violation(
                        "the Media path array index is out of range",
                    ));
                };
                *slot = replacement;
                Ok(())
            }
            _ => Err(meta_path_violation(
                "the Media path traverses a non-container metadata value",
            )),
        }
    } else {
        match current {
            Value::Object(map) => {
                let child = map
                    .entry(segment.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                if child.is_null() {
                    return Err(meta_path_violation(
                        "the Media path traverses a null metadata value",
                    ));
                }
                set_json_pointer_at(child, &segments[1..], replacement)
            }
            Value::Array(array) => {
                let index = array_index(segment)?;
                let Some(child) = array.get_mut(index) else {
                    return Err(meta_path_violation(
                        "the Media path array index is out of range",
                    ));
                };
                set_json_pointer_at(child, &segments[1..], replacement)
            }
            _ => Err(meta_path_violation(
                "the Media path traverses a non-container metadata value",
            )),
        }
    }
}

fn pointer_media_asset_id(root: &Value, segments: &[String]) -> Option<Uuid> {
    let mut current = root;
    for segment in segments {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(array) => array.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    current
        .get("media_asset_id")?
        .as_str()?
        .parse::<Uuid>()
        .ok()
}

fn remove_json_pointer(root: &mut Value, segments: &[String]) -> bool {
    let Some(segment) = segments.first() else {
        return false;
    };
    if segments.len() == 1 {
        return match root {
            Value::Object(map) => map.remove(segment).is_some(),
            Value::Array(array) => match segment.parse::<usize>() {
                Ok(index) => array
                    .get_mut(index)
                    .map(|slot| {
                        *slot = Value::Null;
                        true
                    })
                    .unwrap_or(false),
                Err(_) => false,
            },
            _ => false,
        };
    }
    match root {
        Value::Object(map) => map
            .get_mut(segment)
            .is_some_and(|child| remove_json_pointer(child, &segments[1..])),
        Value::Array(array) => match segment.parse::<usize>() {
            Ok(index) => array
                .get_mut(index)
                .is_some_and(|child| remove_json_pointer(child, &segments[1..])),
            Err(_) => false,
        },
        _ => false,
    }
}

fn clear_media_reference(root: &mut Value, segments: &[String]) -> bool {
    let remove_node = match json_pointer_mut(root, segments) {
        Some(Value::Object(map)) => {
            map.remove("media_asset_id");
            map.remove("alt_text");
            map.remove("media_type");
            map.remove("url");
            map.is_empty()
        }
        Some(_) => true,
        None => return false,
    };
    if remove_node {
        remove_json_pointer(root, segments)
    } else {
        true
    }
}

fn array_index(value: &str) -> Result<usize, ApplicationError> {
    value
        .parse::<usize>()
        .map_err(|_| meta_path_violation("array path segments must be non-negative integers"))
}

fn meta_path_violation(reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field: "meta_path",
            reason: reason.into(),
        }],
    }
}

fn decode_digest(value: &str) -> Result<[u8; 32], ApplicationError> {
    let mut output = [0_u8; 32];
    for (index, slot) in output.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| invalid_snapshot())?;
    }
    Ok(output)
}

fn encode_digest(value: &[u8]) -> Result<String, ApplicationError> {
    if value.len() != 32 {
        return Err(invalid_snapshot());
    }
    Ok(value.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn map_product_media_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(db) = &error
        && db.constraint() == Some("product_media_assets_position_active_idx")
    {
        return ApplicationError::Conflict {
            code: "media_position_taken",
            message: "the Media position is already occupied for this Product",
        };
    }
    database_error(error)
}

fn map_product_option_value_media_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(db) = &error
        && db.constraint() == Some("product_option_value_media_assets_position_active_idx")
    {
        return ApplicationError::Conflict {
            code: "media_position_taken",
            message: "the Media position is already occupied for this Product Option Value",
        };
    }
    database_error(error)
}

fn map_product_variant_media_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(db) = &error
        && db.constraint() == Some("product_variant_media_assets_position_active_idx")
    {
        return ApplicationError::Conflict {
            code: "media_position_taken",
            message: "the Media position is already occupied for this Product Variant",
        };
    }
    database_error(error)
}

fn map_review_media_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(db) = &error
        && db.constraint() == Some("review_media_assets_position_active_idx")
    {
        return ApplicationError::Conflict {
            code: "review_media_position_taken",
            message: "the Media position is already occupied for this Review",
        };
    }
    database_error(error)
}

fn map_product_meta_media_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(db) = &error
        && db.constraint() == Some("product_meta_media_assets_path_active_idx")
    {
        return ApplicationError::Conflict {
            code: "product_meta_media_path_taken",
            message: "the Product metadata path is already occupied by another Media Asset",
        };
    }
    database_error(error)
}

fn not_found(id: MediaAssetId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "media_asset",
        id: id.as_uuid().to_string(),
    }
}

fn not_found_product(id: ProductId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "product",
        id: id.as_uuid().to_string(),
    }
}

fn not_found_review(id: ReviewId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "review",
        id: id.as_uuid().to_string(),
    }
}

fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "the Media Asset persistence snapshot is invalid"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_metadata_pointer_is_set_and_removed_without_inline_media_bytes() {
        let segments = parse_json_pointer("/landing_page/hero/image").unwrap();
        let mut metadata = json!({
            "landing_page": {
                "hero": {
                    "image": {
                        "crop": "cover",
                        "url": "https://stale.example/image.jpg",
                        "media_type": "image/jpeg"
                    }
                }
            }
        });
        let asset_id = Uuid::now_v7();
        set_media_reference(&mut metadata, &segments, asset_id, "Hero").unwrap();

        assert_eq!(pointer_media_asset_id(&metadata, &segments), Some(asset_id));
        assert_eq!(metadata["landing_page"]["hero"]["image"]["crop"], "cover");
        assert!(clear_media_reference(&mut metadata, &segments));
        assert_eq!(
            metadata,
            json!({
                "landing_page": {
                    "hero": {
                        "image": {"crop": "cover"}
                    }
                }
            })
        );
    }

    #[test]
    fn metadata_pointer_rejects_invalid_escape_sequences() {
        assert!(parse_json_pointer("/landing_page/~2image").is_err());
        assert!(parse_json_pointer("/landing_page//image").is_err());
        assert!(parse_json_pointer("landing_page/hero").is_err());
    }
}
