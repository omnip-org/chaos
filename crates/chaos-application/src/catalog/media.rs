use std::sync::Arc;

use chaos_domain::{
    catalog::{MediaAssetId, MediaAssetStatus, MediaDescriptor, ProductId, ProductVariantId},
    store::StoreId,
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    ports::{
        AdminActor, CreateMediaAssetRecord, IdempotencyRequest, MediaAssetItem, MediaAssetMutation,
        MediaAssetRepository, MediaStorage, MediaUploadRequest,
    },
};

pub struct CreateMediaAssetInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: Option<ProductVariantId>,
    pub file_name: String,
    pub media_type: String,
    pub byte_size: u64,
    pub sha256_hex: String,
    pub alt_text: String,
    pub position: u16,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}
pub struct MediaAssetActionInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}
pub struct RefreshMediaUploadInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub now: OffsetDateTime,
}
pub struct CreatedMediaAsset {
    pub asset: MediaAssetItem,
    pub upload: MediaUploadRequest,
}

pub struct MediaAdministration {
    repository: Arc<dyn MediaAssetRepository>,
    storage: Arc<dyn MediaStorage>,
}

impl MediaAdministration {
    pub fn new(repository: Arc<dyn MediaAssetRepository>, storage: Arc<dyn MediaStorage>) -> Self {
        Self {
            repository,
            storage,
        }
    }
    pub async fn create(
        &self,
        input: CreateMediaAssetInput,
    ) -> Result<CreatedMediaAsset, ApplicationError> {
        require_writer(&input.actor)?;
        if input.position > 99 {
            return Err(validation("position", "must be between 0 and 99"));
        }
        let descriptor = MediaDescriptor::new(
            input.file_name,
            input.media_type,
            input.byte_size,
            input.sha256_hex,
            input.alt_text,
        )?;
        let id = MediaAssetId::new();
        let object_key = format!(
            "stores/{}/media/{}/original",
            input.store_id.as_uuid(),
            id.as_uuid()
        );
        let pending = self
            .repository
            .create(
                input.actor,
                CreateMediaAssetRecord {
                    id,
                    store_id: input.store_id,
                    product_id: input.product_id,
                    product_variant_id: input.product_variant_id,
                    descriptor,
                    object_key,
                    position: input.position,
                    created_at: input.now,
                },
                &input.idempotency,
            )
            .await?;
        let upload = self.upload(&pending, input.now).await?;
        Ok(CreatedMediaAsset {
            asset: pending.asset,
            upload,
        })
    }
    pub async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Vec<MediaAssetItem>, ApplicationError> {
        self.repository
            .list(actor, store_id, product_id)
            .await?
            .ok_or_else(|| not_found("product", product_id.as_uuid().to_string()))
    }
    pub async fn refresh_upload(
        &self,
        input: RefreshMediaUploadInput,
    ) -> Result<MediaUploadRequest, ApplicationError> {
        require_writer(&input.actor)?;
        let pending = self
            .repository
            .pending_upload(
                input.actor,
                input.store_id,
                input.product_id,
                input.media_asset_id,
            )
            .await?;
        if pending.asset.status != MediaAssetStatus::PendingUpload {
            return Err(ApplicationError::Conflict {
                code: "media_asset_not_pending",
                message: "upload credentials can be issued only for a pending Media Asset",
            });
        }
        self.upload(&pending, input.now).await
    }
    pub async fn complete(
        &self,
        input: MediaAssetActionInput,
    ) -> Result<MediaAssetItem, ApplicationError> {
        require_writer(&input.actor)?;
        let pending = self
            .repository
            .pending_upload(
                input.actor.clone(),
                input.store_id,
                input.product_id,
                input.media_asset_id,
            )
            .await?;
        if pending.asset.status == MediaAssetStatus::Ready {
            return Ok(pending.asset);
        }
        let stored =
            self.storage
                .inspect(&pending.object_key)
                .await?
                .ok_or(ApplicationError::Conflict {
                    code: "media_upload_missing",
                    message: "the uploaded object is not visible in Media storage",
                })?;
        if stored.media_type != pending.asset.media_type
            || stored.byte_size != pending.asset.byte_size
            || stored.sha256_hex != pending.asset.sha256_hex
        {
            return Err(ApplicationError::Conflict {
                code: "media_upload_mismatch",
                message: "the uploaded object does not match the declared Media metadata",
            });
        }
        let public_url = self.storage.public_url(&pending.object_key)?;
        self.repository
            .mark_ready(
                input.actor,
                MediaAssetMutation {
                    store_id: input.store_id,
                    product_id: input.product_id,
                    media_asset_id: input.media_asset_id,
                    changed_at: input.now,
                },
                &public_url,
                &input.idempotency,
            )
            .await
    }
    pub async fn archive(
        &self,
        input: MediaAssetActionInput,
    ) -> Result<MediaAssetItem, ApplicationError> {
        require_writer(&input.actor)?;
        self.repository
            .archive(
                input.actor,
                MediaAssetMutation {
                    store_id: input.store_id,
                    product_id: input.product_id,
                    media_asset_id: input.media_asset_id,
                    changed_at: input.now,
                },
                &input.idempotency,
            )
            .await
    }
    async fn upload(
        &self,
        pending: &crate::ports::PendingMediaUpload,
        now: OffsetDateTime,
    ) -> Result<MediaUploadRequest, ApplicationError> {
        let descriptor = MediaDescriptor::new(
            pending.asset.file_name.clone(),
            pending.asset.media_type.clone(),
            pending.asset.byte_size,
            pending.asset.sha256_hex.clone(),
            pending.asset.alt_text.clone(),
        )?;
        self.storage
            .prepare_upload(
                &pending.object_key,
                &descriptor,
                std::time::Duration::from_secs(15 * 60),
                now + Duration::minutes(15),
            )
            .await
    }
}

fn require_writer(actor: &AdminActor) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => Ok(()),
        AdminActor::Machine(_) => Err(ApplicationError::Forbidden),
    }
}
fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}
fn not_found(resource: &'static str, id: String) -> ApplicationError {
    ApplicationError::NotFound { resource, id }
}
