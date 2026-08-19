use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, CreateMediaAssetRecord, IdempotencyRequest, MediaAssetItem, MediaAssetMutation,
        MediaAssetRepository, PendingMediaUpload,
    },
};
use chaos_domain::{
    FieldViolation,
    catalog::{MediaAssetId, MediaAssetStatus, MediaKind, ProductId, ProductVariantId},
    merchant::StoreId,
};
use serde_json::json;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE: &str = "media_assets.create.v1";
const COMPLETE: &str = "media_assets.complete.v1";
const ARCHIVE: &str = "media_assets.archive.v1";

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
        sqlx::query("SELECT set_config('app.user_id',$1,true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.store_id',$1,true)")
            .bind(actor.store_id().as_uuid().to_string())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        Ok(tx)
    }
}

#[derive(FromRow)]
struct MediaRow {
    id: Uuid,
    store_id: Uuid,
    product_id: Uuid,
    product_variant_id: Option<Uuid>,
    object_key: String,
    file_name: String,
    media_type: String,
    media_kind: String,
    byte_size: i64,
    sha256_digest: Vec<u8>,
    alt_text: String,
    position: i16,
    status: String,
    public_url: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

#[async_trait]
impl MediaAssetRepository for PostgresMediaAssetRepository {
    async fn create(
        &self,
        actor: AdminActor,
        record: CreateMediaAssetRecord,
        request: &IdempotencyRequest,
    ) -> Result<PendingMediaUpload, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, CREATE, request).await? {
            let row = load(
                &mut tx,
                &actor,
                record.store_id,
                record.product_id,
                MediaAssetId::from_uuid(id),
            )
            .await?
            .ok_or_else(invalid_snapshot)?;
            tx.commit().await.map_err(database_error)?;
            return pending(row);
        }
        let product_exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog.products WHERE store_id=$1 AND id=$2 AND status<>'archived')").bind(record.store_id.as_uuid()).bind(record.product_id.as_uuid()).fetch_one(&mut *tx).await.map_err(database_error)?;
        if !product_exists {
            return Err(ApplicationError::NotFound {
                resource: "product",
                id: record.product_id.as_uuid().to_string(),
            });
        }
        if let Some(variant) = record.product_variant_id {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog.product_variants WHERE store_id=$1 AND product_id=$2 AND id=$3)").bind(record.store_id.as_uuid()).bind(record.product_id.as_uuid()).bind(variant.as_uuid()).fetch_one(&mut *tx).await.map_err(database_error)?;
            if !valid {
                return Err(ApplicationError::Validation {
                    violations: vec![FieldViolation {
                        field: "product_variant_id",
                        reason: "must identify a Variant of the same Product".into(),
                    }],
                });
            }
        }
        let digest = decode_digest(record.descriptor.sha256_hex())?;
        sqlx::query("INSERT INTO catalog.media_assets (id,store_id,product_id,product_variant_id,object_key,file_name,media_type,media_kind,byte_size,sha256_digest,alt_text,position,status,created_by,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8::catalog.media_kind,$9,$10,$11,$12,'pending_upload',$13,$14,$14)")
            .bind(record.id.as_uuid()).bind(record.store_id.as_uuid()).bind(record.product_id.as_uuid()).bind(record.product_variant_id.map(ProductVariantId::as_uuid)).bind(&record.object_key).bind(record.descriptor.file_name()).bind(record.descriptor.media_type()).bind(record.descriptor.kind().as_str()).bind(i64::try_from(record.descriptor.byte_size()).map_err(|_|invalid_snapshot())?).bind(digest.as_slice()).bind(record.descriptor.alt_text()).bind(i16::try_from(record.position).map_err(|_|invalid_snapshot())?).bind(audit_user_id).bind(record.created_at).execute(&mut *tx).await.map_err(map_media_error)?;
        event(
            &mut tx,
            &actor,
            record.store_id,
            record.product_id,
            record.id,
            "created",
            record.created_at,
        )
        .await?;
        complete(&mut tx, &actor, CREATE, request, record.id, 201).await?;
        let row = load(
            &mut tx,
            &actor,
            record.store_id,
            record.product_id,
            record.id,
        )
        .await?
        .ok_or_else(invalid_snapshot)?;
        tx.commit().await.map_err(database_error)?;
        pending(row)
    }

    async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Option<Vec<MediaAssetItem>>, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM catalog.products WHERE store_id=$1 AND id=$2)",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if !exists {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, MediaRow>(
            "SELECT id,store_id,product_id,product_variant_id,object_key,file_name,media_type,media_kind::text,byte_size,sha256_digest,alt_text,position,status::text,public_url,created_at,updated_at FROM catalog.media_assets WHERE store_id=$1 AND product_id=$2 ORDER BY position,id",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn pending_upload(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        media_asset_id: MediaAssetId,
    ) -> Result<PendingMediaUpload, ApplicationError> {
        let mut tx = self.begin(&actor).await?;
        let row = load(&mut tx, &actor, store_id, product_id, media_asset_id)
            .await?
            .ok_or_else(|| not_found(media_asset_id))?;
        tx.commit().await.map_err(database_error)?;
        pending(row)
    }

    async fn mark_ready(
        &self,
        actor: AdminActor,
        mutation: MediaAssetMutation,
        public_url: &str,
        request: &IdempotencyRequest,
    ) -> Result<MediaAssetItem, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, COMPLETE, request).await? {
            let row = load(
                &mut tx,
                &actor,
                mutation.store_id,
                mutation.product_id,
                MediaAssetId::from_uuid(id),
            )
            .await?
            .ok_or_else(invalid_snapshot)?;
            tx.commit().await.map_err(database_error)?;
            return item(row);
        }
        let changed=sqlx::query("UPDATE catalog.media_assets SET status='ready',public_url=$4,ready_by=$5,ready_at=$6,updated_at=$6 WHERE store_id=$1 AND product_id=$2 AND id=$3 AND status='pending_upload'").bind(mutation.store_id.as_uuid()).bind(mutation.product_id.as_uuid()).bind(mutation.media_asset_id.as_uuid()).bind(public_url).bind(audit_user_id).bind(mutation.changed_at).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        let row = load(
            &mut tx,
            &actor,
            mutation.store_id,
            mutation.product_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        if changed == 0 && row.status != "ready" {
            return Err(ApplicationError::Conflict {
                code: "media_asset_not_pending",
                message: "the Media Asset is not pending upload",
            });
        }
        if changed == 1 {
            event(
                &mut tx,
                &actor,
                mutation.store_id,
                mutation.product_id,
                mutation.media_asset_id,
                "ready",
                mutation.changed_at,
            )
            .await?;
        }
        complete(
            &mut tx,
            &actor,
            COMPLETE,
            request,
            mutation.media_asset_id,
            200,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        item(row)
    }

    async fn archive(
        &self,
        actor: AdminActor,
        mutation: MediaAssetMutation,
        request: &IdempotencyRequest,
    ) -> Result<MediaAssetItem, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut tx = self.begin(&actor).await?;
        if let Some(id) = reserve(&mut tx, &actor, ARCHIVE, request).await? {
            let row = load(
                &mut tx,
                &actor,
                mutation.store_id,
                mutation.product_id,
                MediaAssetId::from_uuid(id),
            )
            .await?
            .ok_or_else(invalid_snapshot)?;
            tx.commit().await.map_err(database_error)?;
            return item(row);
        }
        let changed=sqlx::query("UPDATE catalog.media_assets SET status='archived',archived_by=$4,archived_at=$5,updated_at=$5 WHERE store_id=$1 AND product_id=$2 AND id=$3 AND status<>'archived'").bind(mutation.store_id.as_uuid()).bind(mutation.product_id.as_uuid()).bind(mutation.media_asset_id.as_uuid()).bind(audit_user_id).bind(mutation.changed_at).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        let row = load(
            &mut tx,
            &actor,
            mutation.store_id,
            mutation.product_id,
            mutation.media_asset_id,
        )
        .await?
        .ok_or_else(|| not_found(mutation.media_asset_id))?;
        if changed == 1 {
            event(
                &mut tx,
                &actor,
                mutation.store_id,
                mutation.product_id,
                mutation.media_asset_id,
                "archived",
                mutation.changed_at,
            )
            .await?;
        }
        complete(
            &mut tx,
            &actor,
            ARCHIVE,
            request,
            mutation.media_asset_id,
            200,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        item(row)
    }
}

async fn load(
    tx: &mut Transaction<'_, Postgres>,
    _actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    id: MediaAssetId,
) -> Result<Option<MediaRow>, ApplicationError> {
    sqlx::query_as(
        "SELECT id,store_id,product_id,product_variant_id,object_key,file_name,media_type,media_kind::text,byte_size,sha256_digest,alt_text,position,status::text,public_url,created_at,updated_at FROM catalog.media_assets WHERE store_id=$1 AND product_id=$2 AND id=$3",
    )
    .bind(store.as_uuid())
    .bind(product.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)
}
fn item(row: MediaRow) -> Result<MediaAssetItem, ApplicationError> {
    let byte_size = u64::try_from(row.byte_size).map_err(|_| invalid_snapshot())?;
    let position = u16::try_from(row.position).map_err(|_| invalid_snapshot())?;
    let kind = match row.media_kind.as_str() {
        "image" => MediaKind::Image,
        "video" => MediaKind::Video,
        _ => return Err(invalid_snapshot()),
    };
    let status = MediaAssetStatus::parse(&row.status).ok_or_else(invalid_snapshot)?;
    Ok(MediaAssetItem {
        id: MediaAssetId::from_uuid(row.id),
        store_id: StoreId::from_uuid(row.store_id),
        product_id: ProductId::from_uuid(row.product_id),
        product_variant_id: row.product_variant_id.map(ProductVariantId::from_uuid),
        file_name: row.file_name,
        media_type: row.media_type,
        kind,
        byte_size,
        sha256_hex: encode_digest(&row.sha256_digest)?,
        alt_text: row.alt_text,
        position,
        status,
        public_url: row.public_url,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}
fn pending(row: MediaRow) -> Result<PendingMediaUpload, ApplicationError> {
    if row.status == "archived" {
        return Err(ApplicationError::Conflict {
            code: "media_asset_archived",
            message: "an archived Media Asset cannot be uploaded",
        });
    }
    let object_key = row.object_key.clone();
    Ok(PendingMediaUpload {
        asset: item(row)?,
        object_key,
    })
}
async fn event(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    id: MediaAssetId,
    kind: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query("INSERT INTO catalog.media_events (id,store_id,product_id,media_asset_id,event_kind,actor_user_id,occurred_at) VALUES ($1,$2,$3,$4,$5::catalog.media_event_kind,$6,$7)").bind(Uuid::now_v7()).bind(store.as_uuid()).bind(product.as_uuid()).bind(id.as_uuid()).bind(kind).bind(actor.audit_user_id().as_uuid()).bind(now).execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}
async fn reserve(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Uuid>, ApplicationError> {
    let Some(value) = idempotency::reserve(
        tx,
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
        operation,
        request,
    )
    .await?
    else {
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
    id: MediaAssetId,
    status: i16,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        tx,
        &IdempotencyScope::Store(actor.store_id().as_uuid()),
        operation,
        request,
        status,
        json!({"id":id.as_uuid()}),
    )
    .await
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
fn map_media_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(db) = &error
        && db.constraint() == Some("media_assets_product_position_active_idx")
    {
        return ApplicationError::Conflict {
            code: "media_position_taken",
            message: "the Media position is already occupied for this Product",
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
fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "the Media Asset persistence snapshot is invalid"
    ))
}
fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
