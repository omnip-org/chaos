use std::sync::Arc;

use chaos_domain::{
    catalog::{MediaAssetId, MediaAssetStatus, MediaDescriptor, ProductId, ReviewId},
    store::StoreId,
};
use time::{Duration, OffsetDateTime};

use crate::{
    ApplicationError,
    adapters::postgres::PostgresMediaAssetRepository,
    contracts::{
        AdminActor, CreateMediaAssetRecord, MediaAssetItem, MediaAssetMutation,
        MediaAssetStorageRecord, MediaStorage, MediaUploadRequest, ProductMediaAssetItem,
        ProductMediaAssetLinkRecord, ProductMediaAssetMutation, ProductMetaMediaAssetItem,
        ProductMetaMediaAssetLinkRecord, ProductMetaMediaAssetMutation, ReviewMediaAssetItem,
        ReviewMediaAssetLinkRecord, ReviewMediaAssetMutation,
    },
};

pub struct CreateMediaUploadInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub file_name: String,
    pub media_type: String,
    pub byte_size: u64,
    pub sha256_hex: String,
    pub now: OffsetDateTime,
}

pub struct RefreshMediaUploadInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub media_asset_id: MediaAssetId,
    pub now: OffsetDateTime,
}

pub struct CompleteMediaUploadInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub media_asset_id: MediaAssetId,
    pub now: OffsetDateTime,
}

pub struct ArchiveMediaAssetInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub media_asset_id: MediaAssetId,
    pub now: OffsetDateTime,
}

pub struct AttachProductMediaInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub product_variant_id: Option<chaos_domain::catalog::ProductVariantId>,
    pub media_asset_id: MediaAssetId,
    pub alt_text: String,
    pub position: u16,
}

pub struct AttachReviewMediaInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub review_id: ReviewId,
    pub media_asset_id: MediaAssetId,
    pub alt_text: String,
    pub position: u16,
}

pub struct AttachProductMetaMediaInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    /// RFC 6901 JSON Pointer into the Product metadata object.
    pub meta_path: String,
    pub alt_text: String,
    pub now: OffsetDateTime,
}

pub struct ArchiveProductMediaInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub now: OffsetDateTime,
}

pub struct ArchiveReviewMediaInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub review_id: ReviewId,
    pub media_asset_id: MediaAssetId,
    pub now: OffsetDateTime,
}

pub struct ArchiveProductMetaMediaInput {
    pub actor: AdminActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub meta_path: String,
    pub now: OffsetDateTime,
}

pub struct CreatedMediaAsset {
    pub asset: MediaAssetItem,
    pub upload: MediaUploadRequest,
}

pub struct MediaAdministration {
    repository: Arc<PostgresMediaAssetRepository>,
    storage: Arc<dyn MediaStorage>,
}

impl MediaAdministration {
    pub fn new(
        repository: Arc<PostgresMediaAssetRepository>,
        storage: Arc<dyn MediaStorage>,
    ) -> Self {
        Self {
            repository,
            storage,
        }
    }

    /// Creates only the reusable physical asset. No business relation is implied
    /// until a typed attachment operation is called.
    pub async fn create(
        &self,
        input: CreateMediaUploadInput,
    ) -> Result<CreatedMediaAsset, ApplicationError> {
        input.actor.require_human()?;
        let CreateMediaUploadInput {
            actor,
            store_id,
            file_name,
            media_type,
            byte_size,
            sha256_hex,
            now,
        } = input;
        let descriptor = MediaDescriptor::new(file_name, media_type, byte_size, sha256_hex, "")?;
        let id = MediaAssetId::new();
        let object_key = object_key(store_id, id);
        let record = self
            .repository
            .create_asset(
                actor,
                CreateMediaAssetRecord {
                    id,
                    store_id,
                    descriptor,
                    object_key,
                    created_at: now,
                },
            )
            .await?;
        let upload = self.upload(&record, now).await?;
        Ok(CreatedMediaAsset {
            asset: record.asset,
            upload,
        })
    }

    pub async fn refresh_upload(
        &self,
        input: RefreshMediaUploadInput,
    ) -> Result<MediaUploadRequest, ApplicationError> {
        input.actor.require_human()?;
        let record = self
            .repository
            .asset(input.actor, input.store_id, input.media_asset_id)
            .await?;
        self.refresh_pending(record, input.now).await
    }

    pub async fn complete_upload(
        &self,
        input: CompleteMediaUploadInput,
    ) -> Result<MediaAssetItem, ApplicationError> {
        input.actor.require_human()?;
        let record = self
            .repository
            .asset(input.actor.clone(), input.store_id, input.media_asset_id)
            .await?;
        if record.asset.status == MediaAssetStatus::Ready {
            return Ok(record.asset);
        }
        if record.asset.status == MediaAssetStatus::Archived {
            return Err(ApplicationError::Conflict {
                code: "media_asset_archived",
                message: "an archived Media Asset cannot be completed",
            });
        }
        self.complete_pending(&record, input.actor, input.store_id, input.now)
            .await
    }

    pub async fn get(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        media_asset_id: MediaAssetId,
    ) -> Result<MediaAssetItem, ApplicationError> {
        self.repository
            .asset(actor, store_id, media_asset_id)
            .await
            .map(|record| record.asset)
    }

    pub async fn archive(
        &self,
        input: ArchiveMediaAssetInput,
    ) -> Result<MediaAssetItem, ApplicationError> {
        input.actor.require_human()?;
        self.repository
            .archive_asset(
                input.actor,
                MediaAssetMutation {
                    store_id: input.store_id,
                    media_asset_id: input.media_asset_id,
                    changed_at: input.now,
                },
            )
            .await
    }

    pub async fn attach_product(
        &self,
        input: AttachProductMediaInput,
    ) -> Result<ProductMediaAssetItem, ApplicationError> {
        input.actor.require_human()?;
        validate_position(input.position)?;
        validate_alt_text(&input.alt_text)?;
        self.repository
            .attach_product(
                input.actor,
                ProductMediaAssetLinkRecord {
                    store_id: input.store_id,
                    product_id: input.product_id,
                    product_variant_id: input.product_variant_id,
                    media_asset_id: input.media_asset_id,
                    alt_text: input.alt_text,
                    position: input.position,
                },
            )
            .await
    }

    pub async fn attach_review(
        &self,
        input: AttachReviewMediaInput,
    ) -> Result<ReviewMediaAssetItem, ApplicationError> {
        input.actor.require_human()?;
        validate_position(input.position)?;
        validate_alt_text(&input.alt_text)?;
        self.repository
            .attach_review(
                input.actor,
                ReviewMediaAssetLinkRecord {
                    store_id: input.store_id,
                    review_id: input.review_id,
                    media_asset_id: input.media_asset_id,
                    alt_text: input.alt_text,
                    position: input.position,
                },
            )
            .await
    }

    pub async fn attach_product_meta(
        &self,
        input: AttachProductMetaMediaInput,
    ) -> Result<ProductMetaMediaAssetItem, ApplicationError> {
        input.actor.require_human()?;
        validate_alt_text(&input.alt_text)?;
        crate::catalog::parse_json_pointer(&input.meta_path)?;
        self.repository
            .attach_product_meta(
                input.actor,
                ProductMetaMediaAssetLinkRecord {
                    store_id: input.store_id,
                    product_id: input.product_id,
                    media_asset_id: input.media_asset_id,
                    meta_path: input.meta_path,
                    alt_text: input.alt_text,
                    changed_at: input.now,
                },
            )
            .await
    }

    pub async fn list_product(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Vec<ProductMediaAssetItem>, ApplicationError> {
        self.repository
            .list_product(actor, store_id, product_id)
            .await?
            .ok_or_else(|| not_found("product", product_id.as_uuid().to_string()))
    }

    pub async fn list_review(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        review_id: ReviewId,
    ) -> Result<Vec<ReviewMediaAssetItem>, ApplicationError> {
        self.repository
            .list_review(actor, store_id, review_id)
            .await?
            .ok_or_else(|| not_found("review", review_id.as_uuid().to_string()))
    }

    pub async fn list_product_meta(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Vec<ProductMetaMediaAssetItem>, ApplicationError> {
        self.repository
            .list_product_meta(actor, store_id, product_id)
            .await?
            .ok_or_else(|| not_found("product", product_id.as_uuid().to_string()))
    }

    pub async fn archive_product(
        &self,
        input: ArchiveProductMediaInput,
    ) -> Result<ProductMediaAssetItem, ApplicationError> {
        input.actor.require_human()?;
        self.repository
            .archive_product(
                input.actor,
                ProductMediaAssetMutation {
                    store_id: input.store_id,
                    product_id: input.product_id,
                    media_asset_id: input.media_asset_id,
                    changed_at: input.now,
                },
            )
            .await
    }

    pub async fn archive_review(
        &self,
        input: ArchiveReviewMediaInput,
    ) -> Result<ReviewMediaAssetItem, ApplicationError> {
        input.actor.require_human()?;
        self.repository
            .archive_review(
                input.actor,
                ReviewMediaAssetMutation {
                    store_id: input.store_id,
                    review_id: input.review_id,
                    media_asset_id: input.media_asset_id,
                    changed_at: input.now,
                },
            )
            .await
    }

    pub async fn archive_product_meta(
        &self,
        input: ArchiveProductMetaMediaInput,
    ) -> Result<ProductMetaMediaAssetItem, ApplicationError> {
        input.actor.require_human()?;
        crate::catalog::parse_json_pointer(&input.meta_path)?;
        self.repository
            .archive_product_meta(
                input.actor,
                ProductMetaMediaAssetMutation {
                    store_id: input.store_id,
                    product_id: input.product_id,
                    media_asset_id: input.media_asset_id,
                    meta_path: input.meta_path,
                    changed_at: input.now,
                },
            )
            .await
    }

    async fn refresh_pending(
        &self,
        record: MediaAssetStorageRecord,
        now: OffsetDateTime,
    ) -> Result<MediaUploadRequest, ApplicationError> {
        if record.asset.status != MediaAssetStatus::PendingUpload {
            return Err(ApplicationError::Conflict {
                code: "media_asset_not_pending",
                message: "upload credentials can be issued only for a pending Media Asset",
            });
        }
        self.upload(&record, now).await
    }

    async fn complete_pending(
        &self,
        record: &MediaAssetStorageRecord,
        actor: AdminActor,
        store_id: StoreId,
        now: OffsetDateTime,
    ) -> Result<MediaAssetItem, ApplicationError> {
        let stored =
            self.storage
                .inspect(&record.object_key)
                .await?
                .ok_or(ApplicationError::Conflict {
                    code: "media_upload_missing",
                    message: "the uploaded object is not visible in Media storage",
                })?;
        if stored.media_type != record.asset.media_type
            || stored.byte_size != record.asset.byte_size
            || stored.sha256_hex != record.asset.sha256_hex
        {
            return Err(ApplicationError::Conflict {
                code: "media_upload_mismatch",
                message: "the uploaded object does not match the declared Media metadata",
            });
        }
        let public_url = self.storage.public_url(&record.object_key)?;
        self.repository
            .mark_ready(
                actor,
                MediaAssetMutation {
                    store_id,
                    media_asset_id: record.asset.id,
                    changed_at: now,
                },
                &public_url,
            )
            .await
    }

    async fn upload(
        &self,
        record: &MediaAssetStorageRecord,
        now: OffsetDateTime,
    ) -> Result<MediaUploadRequest, ApplicationError> {
        let descriptor = MediaDescriptor::new(
            record.asset.file_name.clone(),
            record.asset.media_type.clone(),
            record.asset.byte_size,
            record.asset.sha256_hex.clone(),
            "",
        )?;
        self.storage
            .prepare_upload(
                &record.object_key,
                &descriptor,
                std::time::Duration::from_secs(15 * 60),
                now + Duration::minutes(15),
            )
            .await
    }
}

fn object_key(store_id: StoreId, media_asset_id: MediaAssetId) -> String {
    format!(
        "stores/{}/media/{}/original",
        store_id.as_uuid(),
        media_asset_id.as_uuid()
    )
}

fn validate_position(position: u16) -> Result<(), ApplicationError> {
    if position > 99 {
        return Err(validation("position", "must be between 0 and 99"));
    }
    Ok(())
}

fn validate_alt_text(value: &str) -> Result<(), ApplicationError> {
    if value.chars().count() > 500 || value.chars().any(char::is_control) {
        return Err(validation(
            "alt_text",
            "must contain at most 500 non-control characters",
        ));
    }
    Ok(())
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
