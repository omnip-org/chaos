use std::{collections::HashSet, sync::Arc};

use chaos_domain::{
    Locale,
    catalog::{
        CollectionId, LocalizedAltText, LocalizedContent, LocalizedTitle, MediaAssetId, ProductId,
        ProductVariantId,
    },
    merchant::{MerchantRole, StoreId},
};
use time::OffsetDateTime;

use crate::{
    ApplicationError,
    merchant::MerchantActor,
    ports::{
        CatalogLocalizationRepository, CollectionTranslation, IdempotencyRequest, MediaTranslation,
        ProductTranslation, ProductVariantTranslation, StoreLocaleConfiguration,
    },
};

pub struct StoreLocaleInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub locale: String,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct ProductVariantTranslationInput {
    pub product_variant_id: ProductVariantId,
    pub title: String,
}

pub struct UpsertProductTranslationInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub locale: String,
    pub title: String,
    pub description: String,
    pub variants: Vec<ProductVariantTranslationInput>,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct TranslationActionInput<T> {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub resource_id: T,
    pub locale: String,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct UpsertCollectionTranslationInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub collection_id: CollectionId,
    pub locale: String,
    pub title: String,
    pub description: String,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct UpsertMediaTranslationInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub locale: String,
    pub alt_text: String,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct MediaTranslationActionInput {
    pub actor: MerchantActor,
    pub store_id: StoreId,
    pub product_id: ProductId,
    pub media_asset_id: MediaAssetId,
    pub locale: String,
    pub idempotency: IdempotencyRequest,
    pub now: OffsetDateTime,
}

pub struct CatalogLocalization {
    repository: Arc<dyn CatalogLocalizationRepository>,
}

impl CatalogLocalization {
    pub fn new(repository: Arc<dyn CatalogLocalizationRepository>) -> Self {
        Self { repository }
    }

    pub async fn store_locales(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<StoreLocaleConfiguration, ApplicationError> {
        self.repository
            .store_locales(actor, store_id)
            .await?
            .ok_or_else(|| store_not_found(store_id))
    }

    pub async fn enable_locale(
        &self,
        input: StoreLocaleInput,
    ) -> Result<StoreLocaleConfiguration, ApplicationError> {
        require_locale_administrator(input.actor)?;
        self.repository
            .enable_locale(
                input.actor,
                input.store_id,
                &Locale::parse(input.locale)?,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn set_default_locale(
        &self,
        input: StoreLocaleInput,
    ) -> Result<StoreLocaleConfiguration, ApplicationError> {
        require_locale_administrator(input.actor)?;
        self.repository
            .set_default_locale(
                input.actor,
                input.store_id,
                &Locale::parse(input.locale)?,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn disable_locale(
        &self,
        input: StoreLocaleInput,
    ) -> Result<StoreLocaleConfiguration, ApplicationError> {
        require_locale_administrator(input.actor)?;
        self.repository
            .disable_locale(
                input.actor,
                input.store_id,
                &Locale::parse(input.locale)?,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn product_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        product_id: ProductId,
        locale: &str,
    ) -> Result<ProductTranslation, ApplicationError> {
        let locale = Locale::parse(locale)?;
        self.repository
            .product_translation(actor, store_id, product_id, &locale)
            .await?
            .ok_or_else(|| translation_not_found(locale.as_str()))
    }

    pub async fn upsert_product_translation(
        &self,
        input: UpsertProductTranslationInput,
    ) -> Result<ProductTranslation, ApplicationError> {
        require_translation_writer(input.actor)?;
        let locale = Locale::parse(input.locale)?;
        let mut seen = HashSet::with_capacity(input.variants.len());
        let variants = input
            .variants
            .into_iter()
            .map(|variant| {
                if !seen.insert(variant.product_variant_id) {
                    return Err(validation(
                        "variants",
                        "must not contain duplicate Product Variants",
                    ));
                }
                Ok(ProductVariantTranslation {
                    product_variant_id: variant.product_variant_id,
                    title: LocalizedTitle::new(variant.title)?,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        let translation = ProductTranslation {
            content: LocalizedContent::new(locale, input.title, input.description)?,
            variants,
            created_at: input.now,
            updated_at: input.now,
        };
        self.repository
            .upsert_product_translation(
                input.actor,
                input.store_id,
                input.product_id,
                &translation,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn remove_product_translation(
        &self,
        input: TranslationActionInput<ProductId>,
    ) -> Result<(), ApplicationError> {
        require_translation_writer(input.actor)?;
        self.repository
            .remove_product_translation(
                input.actor,
                input.store_id,
                input.resource_id,
                &Locale::parse(input.locale)?,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn upsert_collection_translation(
        &self,
        input: UpsertCollectionTranslationInput,
    ) -> Result<CollectionTranslation, ApplicationError> {
        require_translation_writer(input.actor)?;
        let content =
            LocalizedContent::new(Locale::parse(input.locale)?, input.title, input.description)?;
        self.repository
            .upsert_collection_translation(
                input.actor,
                input.store_id,
                input.collection_id,
                &content,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn collection_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        collection_id: CollectionId,
        locale: &str,
    ) -> Result<CollectionTranslation, ApplicationError> {
        let locale = Locale::parse(locale)?;
        self.repository
            .collection_translation(actor, store_id, collection_id, &locale)
            .await?
            .ok_or_else(|| translation_not_found(locale.as_str()))
    }

    pub async fn remove_collection_translation(
        &self,
        input: TranslationActionInput<CollectionId>,
    ) -> Result<(), ApplicationError> {
        require_translation_writer(input.actor)?;
        self.repository
            .remove_collection_translation(
                input.actor,
                input.store_id,
                input.resource_id,
                &Locale::parse(input.locale)?,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn upsert_media_translation(
        &self,
        input: UpsertMediaTranslationInput,
    ) -> Result<MediaTranslation, ApplicationError> {
        require_translation_writer(input.actor)?;
        let locale = Locale::parse(input.locale)?;
        let alt_text = LocalizedAltText::new(input.alt_text)?;
        self.repository
            .upsert_media_translation(
                input.actor,
                input.store_id,
                input.product_id,
                input.media_asset_id,
                &locale,
                &alt_text,
                &input.idempotency,
                input.now,
            )
            .await
    }

    pub async fn media_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        product_id: ProductId,
        media_asset_id: MediaAssetId,
        locale: &str,
    ) -> Result<MediaTranslation, ApplicationError> {
        let locale = Locale::parse(locale)?;
        self.repository
            .media_translation(actor, store_id, product_id, media_asset_id, &locale)
            .await?
            .ok_or_else(|| translation_not_found(locale.as_str()))
    }

    pub async fn remove_media_translation(
        &self,
        input: MediaTranslationActionInput,
    ) -> Result<(), ApplicationError> {
        require_translation_writer(input.actor)?;
        self.repository
            .remove_media_translation(
                input.actor,
                input.store_id,
                input.product_id,
                input.media_asset_id,
                &Locale::parse(input.locale)?,
                &input.idempotency,
                input.now,
            )
            .await
    }
}

fn require_locale_administrator(actor: MerchantActor) -> Result<(), ApplicationError> {
    if matches!(
        actor.role(),
        MerchantRole::Owner | MerchantRole::Administrator
    ) {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn require_translation_writer(actor: MerchantActor) -> Result<(), ApplicationError> {
    if matches!(
        actor.role(),
        MerchantRole::Owner
            | MerchantRole::Administrator
            | MerchantRole::Developer
            | MerchantRole::Manager
    ) {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn store_not_found(id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: id.as_uuid().to_string(),
    }
}
fn translation_not_found(locale: &str) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "translation",
        id: locale.into(),
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
