use async_trait::async_trait;
use chaos_domain::{
    Locale,
    catalog::{
        CollectionId, LocalizedAltText, LocalizedContent, LocalizedTitle, MediaAssetId, ProductId,
        ProductVariantId,
    },
    merchant::StoreId,
};
use time::OffsetDateTime;

use crate::{ApplicationError, merchant::MerchantActor};

use super::IdempotencyRequest;

pub struct StoreLocaleConfiguration {
    pub default_locale: Locale,
    pub enabled_locales: Vec<Locale>,
}

pub struct ProductVariantTranslation {
    pub product_variant_id: ProductVariantId,
    pub title: LocalizedTitle,
}

pub struct ProductTranslation {
    pub content: LocalizedContent,
    pub variants: Vec<ProductVariantTranslation>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct CollectionTranslation {
    pub content: LocalizedContent,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

pub struct MediaTranslation {
    pub locale: Locale,
    pub alt_text: LocalizedAltText,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[async_trait]
pub trait CatalogLocalizationRepository: Send + Sync {
    async fn store_locales(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
    ) -> Result<Option<StoreLocaleConfiguration>, ApplicationError>;

    async fn enable_locale(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreLocaleConfiguration, ApplicationError>;

    async fn set_default_locale(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreLocaleConfiguration, ApplicationError>;

    async fn disable_locale(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreLocaleConfiguration, ApplicationError>;

    async fn product_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        product_id: ProductId,
        locale: &Locale,
    ) -> Result<Option<ProductTranslation>, ApplicationError>;

    async fn upsert_product_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        product_id: ProductId,
        translation: &ProductTranslation,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<ProductTranslation, ApplicationError>;

    async fn remove_product_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        product_id: ProductId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    async fn upsert_collection_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        collection_id: CollectionId,
        content: &LocalizedContent,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<CollectionTranslation, ApplicationError>;

    async fn collection_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        collection_id: CollectionId,
        locale: &Locale,
    ) -> Result<Option<CollectionTranslation>, ApplicationError>;

    async fn remove_collection_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        collection_id: CollectionId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;

    #[allow(clippy::too_many_arguments)]
    async fn upsert_media_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        product_id: ProductId,
        media_asset_id: MediaAssetId,
        locale: &Locale,
        alt_text: &LocalizedAltText,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<MediaTranslation, ApplicationError>;

    async fn media_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        product_id: ProductId,
        media_asset_id: MediaAssetId,
        locale: &Locale,
    ) -> Result<Option<MediaTranslation>, ApplicationError>;

    #[allow(clippy::too_many_arguments)]
    async fn remove_media_translation(
        &self,
        actor: MerchantActor,
        store_id: StoreId,
        product_id: ProductId,
        media_asset_id: MediaAssetId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError>;
}
