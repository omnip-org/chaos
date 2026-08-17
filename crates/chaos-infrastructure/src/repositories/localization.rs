use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, CatalogLocalizationRepository, CollectionTranslation, IdempotencyRequest,
        MediaTranslation, ProductTranslation, ProductVariantTranslation, StoreLocaleConfiguration,
    },
};
use chaos_domain::{
    Locale,
    catalog::{
        CollectionId, LocalizedAltText, LocalizedContent, LocalizedTitle, MediaAssetId, ProductId,
        ProductVariantId,
    },
    merchant::StoreId,
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const ENABLE_LOCALE: &str = "localization.enable_locale.v1";
const SET_DEFAULT_LOCALE: &str = "localization.set_default_locale.v1";
const DISABLE_LOCALE: &str = "localization.disable_locale.v1";
const UPSERT_PRODUCT: &str = "localization.upsert_product.v1";
const REMOVE_PRODUCT: &str = "localization.remove_product.v1";
const UPSERT_COLLECTION: &str = "localization.upsert_collection.v1";
const REMOVE_COLLECTION: &str = "localization.remove_collection.v1";
const UPSERT_MEDIA: &str = "localization.upsert_media.v1";
const REMOVE_MEDIA: &str = "localization.remove_media.v1";

#[derive(Clone)]
pub struct PostgresCatalogLocalizationRepository {
    pool: PgPool,
}

impl PostgresCatalogLocalizationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT set_config('app.user_id',$1,true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        sqlx::query("SELECT set_config('app.merchant_account_id',$1,true)")
            .bind(actor.merchant_account_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }
}

#[async_trait]
impl CatalogLocalizationRepository for PostgresCatalogLocalizationRepository {
    async fn store_locales(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Option<StoreLocaleConfiguration>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let result = load_store_locales(&mut transaction, &actor, store_id).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn enable_locale(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreLocaleConfiguration, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, ENABLE_LOCALE, request).await? {
            return replay_store_locales(&mut transaction, &actor, store_id).await;
        }
        require_store(&mut transaction, &actor, store_id).await?;
        let inserted = sqlx::query(
            "INSERT INTO merchant.store_locales (merchant_account_id,store_id,locale,created_by_user_id,created_at) VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
        )
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store_id.as_uuid())
        .bind(locale.as_str())
        .bind(audit_user_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        if inserted == 1 {
            locale_event(
                &mut transaction,
                &actor,
                store_id,
                locale,
                None,
                "enabled",
                now,
            )
            .await?;
        }
        complete(&mut transaction, &actor, ENABLE_LOCALE, request).await?;
        let result = load_store_locales(&mut transaction, &actor, store_id)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn set_default_locale(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreLocaleConfiguration, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, SET_DEFAULT_LOCALE, request).await? {
            return replay_store_locales(&mut transaction, &actor, store_id).await;
        }
        let previous = lock_default_locale(&mut transaction, &actor, store_id).await?;
        let inserted = sqlx::query("INSERT INTO merchant.store_locales (merchant_account_id,store_id,locale,created_by_user_id,created_at) VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(locale.as_str()).bind(audit_user_id).bind(now)
            .execute(&mut *transaction).await.map_err(database_error)?.rows_affected();
        if inserted == 1 {
            locale_event(
                &mut transaction,
                &actor,
                store_id,
                locale,
                None,
                "enabled",
                now,
            )
            .await?;
        }
        if previous != locale.as_str() {
            sqlx::query("UPDATE merchant.stores SET default_locale=$3,updated_at=$4 WHERE merchant_account_id=$1 AND id=$2")
                .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(locale.as_str()).bind(now)
                .execute(&mut *transaction).await.map_err(database_error)?;
            locale_event(
                &mut transaction,
                &actor,
                store_id,
                locale,
                Some(&previous),
                "default_changed",
                now,
            )
            .await?;
        }
        complete(&mut transaction, &actor, SET_DEFAULT_LOCALE, request).await?;
        let result = load_store_locales(&mut transaction, &actor, store_id)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn disable_locale(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<StoreLocaleConfiguration, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, DISABLE_LOCALE, request).await? {
            return replay_store_locales(&mut transaction, &actor, store_id).await;
        }
        let default = lock_default_locale(&mut transaction, &actor, store_id).await?;
        if default == locale.as_str() {
            return Err(ApplicationError::Conflict {
                code: "default_locale_cannot_be_disabled",
                message: "the default Store Locale cannot be disabled",
            });
        }
        let deleted = sqlx::query("DELETE FROM merchant.store_locales WHERE merchant_account_id=$1 AND store_id=$2 AND locale=$3")
            .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(locale.as_str())
            .execute(&mut *transaction).await.map_err(database_error)?.rows_affected();
        if deleted == 1 {
            locale_event(
                &mut transaction,
                &actor,
                store_id,
                locale,
                None,
                "disabled",
                now,
            )
            .await?;
        }
        complete(&mut transaction, &actor, DISABLE_LOCALE, request).await?;
        let result = load_store_locales(&mut transaction, &actor, store_id)
            .await?
            .ok_or_else(|| store_not_found(store_id))?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn product_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        locale: &Locale,
    ) -> Result<Option<ProductTranslation>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let result =
            load_product_translation(&mut transaction, &actor, store_id, product_id, locale).await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn upsert_product_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        translation: &ProductTranslation,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<ProductTranslation, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, UPSERT_PRODUCT, request).await? {
            return replay_product_translation(
                &mut transaction,
                &actor,
                store_id,
                product_id,
                translation.content.locale(),
            )
            .await;
        }
        require_translatable_locale(
            &mut transaction,
            &actor,
            store_id,
            translation.content.locale(),
        )
        .await?;
        require_product(&mut transaction, &actor, store_id, product_id).await?;
        require_variants(
            &mut transaction,
            &actor,
            store_id,
            product_id,
            &translation.variants,
        )
        .await?;
        sqlx::query("INSERT INTO catalog.product_translations (merchant_account_id,store_id,product_id,locale,title,description,updated_by_user_id,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8) ON CONFLICT (merchant_account_id,store_id,product_id,locale) DO UPDATE SET title=EXCLUDED.title,description=EXCLUDED.description,updated_by_user_id=EXCLUDED.updated_by_user_id,updated_at=EXCLUDED.updated_at")
            .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(product_id.as_uuid()).bind(translation.content.locale().as_str()).bind(translation.content.title()).bind(translation.content.description()).bind(audit_user_id).bind(now)
            .execute(&mut *transaction).await.map_err(database_error)?;
        sqlx::query("DELETE FROM catalog.product_variant_translations WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 AND locale=$4")
            .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(product_id.as_uuid()).bind(translation.content.locale().as_str())
            .execute(&mut *transaction).await.map_err(database_error)?;
        for variant in &translation.variants {
            sqlx::query("INSERT INTO catalog.product_variant_translations (merchant_account_id,store_id,product_id,product_variant_id,locale,title,updated_by_user_id,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8)")
                .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(product_id.as_uuid()).bind(variant.product_variant_id.as_uuid()).bind(translation.content.locale().as_str()).bind(variant.title.as_str()).bind(audit_user_id).bind(now)
                .execute(&mut *transaction).await.map_err(database_error)?;
        }
        product_event(
            &mut transaction,
            &actor,
            store_id,
            product_id,
            translation.content.locale(),
            "upserted",
            now,
        )
        .await?;
        complete(&mut transaction, &actor, UPSERT_PRODUCT, request).await?;
        let result = load_product_translation(
            &mut transaction,
            &actor,
            store_id,
            product_id,
            translation.content.locale(),
        )
        .await?
        .ok_or_else(invalid_snapshot)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn remove_product_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, REMOVE_PRODUCT, request).await? {
            return Ok(());
        }
        require_product(&mut transaction, &actor, store_id, product_id).await?;
        let deleted = sqlx::query("DELETE FROM catalog.product_translations WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 AND locale=$4")
            .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(product_id.as_uuid()).bind(locale.as_str())
            .execute(&mut *transaction).await.map_err(database_error)?.rows_affected();
        if deleted == 1 {
            product_event(
                &mut transaction,
                &actor,
                store_id,
                product_id,
                locale,
                "removed",
                now,
            )
            .await?;
        }
        complete(&mut transaction, &actor, REMOVE_PRODUCT, request).await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn collection_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
        locale: &Locale,
    ) -> Result<Option<CollectionTranslation>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let result =
            load_collection_translation(&mut transaction, &actor, store_id, collection_id, locale)
                .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn upsert_collection_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
        content: &LocalizedContent,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<CollectionTranslation, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, UPSERT_COLLECTION, request).await? {
            return replay_collection_translation(
                &mut transaction,
                &actor,
                store_id,
                collection_id,
                content.locale(),
            )
            .await;
        }
        require_translatable_locale(&mut transaction, &actor, store_id, content.locale()).await?;
        require_collection(&mut transaction, &actor, store_id, collection_id).await?;
        sqlx::query("INSERT INTO catalog.collection_translations (merchant_account_id,store_id,collection_id,locale,title,description,updated_by_user_id,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8) ON CONFLICT (merchant_account_id,store_id,collection_id,locale) DO UPDATE SET title=EXCLUDED.title,description=EXCLUDED.description,updated_by_user_id=EXCLUDED.updated_by_user_id,updated_at=EXCLUDED.updated_at")
            .bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(content.locale().as_str()).bind(content.title()).bind(content.description()).bind(audit_user_id).bind(now)
            .execute(&mut *transaction).await.map_err(database_error)?;
        collection_event(
            &mut transaction,
            &actor,
            store_id,
            collection_id,
            content.locale(),
            "upserted",
            now,
        )
        .await?;
        complete(&mut transaction, &actor, UPSERT_COLLECTION, request).await?;
        let result = load_collection_translation(
            &mut transaction,
            &actor,
            store_id,
            collection_id,
            content.locale(),
        )
        .await?
        .ok_or_else(invalid_snapshot)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn remove_collection_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        collection_id: CollectionId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, REMOVE_COLLECTION, request).await? {
            return Ok(());
        }
        require_collection(&mut transaction, &actor, store_id, collection_id).await?;
        let deleted=sqlx::query("DELETE FROM catalog.collection_translations WHERE merchant_account_id=$1 AND store_id=$2 AND collection_id=$3 AND locale=$4").bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(collection_id.as_uuid()).bind(locale.as_str()).execute(&mut *transaction).await.map_err(database_error)?.rows_affected();
        if deleted == 1 {
            collection_event(
                &mut transaction,
                &actor,
                store_id,
                collection_id,
                locale,
                "removed",
                now,
            )
            .await?;
        }
        complete(&mut transaction, &actor, REMOVE_COLLECTION, request).await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn media_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        media_asset_id: MediaAssetId,
        locale: &Locale,
    ) -> Result<Option<MediaTranslation>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let result = load_media_translation(
            &mut transaction,
            &actor,
            store_id,
            product_id,
            media_asset_id,
            locale,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn upsert_media_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        media_asset_id: MediaAssetId,
        locale: &Locale,
        alt_text: &LocalizedAltText,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<MediaTranslation, ApplicationError> {
        let audit_user_id = actor.audit_user_id().as_uuid();
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, UPSERT_MEDIA, request).await? {
            return replay_media_translation(
                &mut transaction,
                &actor,
                store_id,
                product_id,
                media_asset_id,
                locale,
            )
            .await;
        }
        require_translatable_locale(&mut transaction, &actor, store_id, locale).await?;
        require_media(
            &mut transaction,
            &actor,
            store_id,
            product_id,
            media_asset_id,
        )
        .await?;
        sqlx::query("INSERT INTO catalog.media_asset_translations (merchant_account_id,store_id,product_id,media_asset_id,locale,alt_text,updated_by_user_id,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8) ON CONFLICT (merchant_account_id,store_id,product_id,media_asset_id,locale) DO UPDATE SET alt_text=EXCLUDED.alt_text,updated_by_user_id=EXCLUDED.updated_by_user_id,updated_at=EXCLUDED.updated_at").bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(product_id.as_uuid()).bind(media_asset_id.as_uuid()).bind(locale.as_str()).bind(alt_text.as_str()).bind(audit_user_id).bind(now).execute(&mut *transaction).await.map_err(database_error)?;
        media_event(
            &mut transaction,
            &actor,
            store_id,
            product_id,
            media_asset_id,
            locale,
            "upserted",
            now,
        )
        .await?;
        complete(&mut transaction, &actor, UPSERT_MEDIA, request).await?;
        let result = load_media_translation(
            &mut transaction,
            &actor,
            store_id,
            product_id,
            media_asset_id,
            locale,
        )
        .await?
        .ok_or_else(invalid_snapshot)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(result)
    }

    async fn remove_media_translation(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
        media_asset_id: MediaAssetId,
        locale: &Locale,
        request: &IdempotencyRequest,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if reserve(&mut transaction, &actor, REMOVE_MEDIA, request).await? {
            return Ok(());
        }
        require_media(
            &mut transaction,
            &actor,
            store_id,
            product_id,
            media_asset_id,
        )
        .await?;
        let deleted=sqlx::query("DELETE FROM catalog.media_asset_translations WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 AND media_asset_id=$4 AND locale=$5").bind(actor.merchant_account_id().as_uuid()).bind(store_id.as_uuid()).bind(product_id.as_uuid()).bind(media_asset_id.as_uuid()).bind(locale.as_str()).execute(&mut *transaction).await.map_err(database_error)?.rows_affected();
        if deleted == 1 {
            media_event(
                &mut transaction,
                &actor,
                store_id,
                product_id,
                media_asset_id,
                locale,
                "removed",
                now,
            )
            .await?;
        }
        complete(&mut transaction, &actor, REMOVE_MEDIA, request).await?;
        transaction.commit().await.map_err(database_error)
    }
}

async fn load_store_locales(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
) -> Result<Option<StoreLocaleConfiguration>, ApplicationError> {
    let default: Option<String> = sqlx::query_scalar(
        "SELECT default_locale FROM merchant.stores WHERE merchant_account_id=$1 AND id=$2",
    )
    .bind(actor.merchant_account_id().as_uuid())
    .bind(store.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?;
    let Some(default) = default else {
        return Ok(None);
    };
    let mut values:Vec<String>=sqlx::query_scalar("SELECT locale FROM merchant.store_locales WHERE merchant_account_id=$1 AND store_id=$2 ORDER BY locale").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).fetch_all(&mut **tx).await.map_err(database_error)?;
    if !values.iter().any(|value| value == &default) {
        values.push(default.clone());
        values.sort();
    }
    Ok(Some(StoreLocaleConfiguration {
        default_locale: parse_locale(&default)?,
        enabled_locales: values
            .into_iter()
            .map(|value| parse_locale(&value))
            .collect::<Result<_, _>>()?,
    }))
}
async fn replay_store_locales(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    store: StoreId,
) -> Result<StoreLocaleConfiguration, ApplicationError> {
    load_store_locales(tx, actor, store)
        .await?
        .ok_or_else(|| store_not_found(store))
}
async fn lock_default_locale(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
) -> Result<String, ApplicationError> {
    sqlx::query_scalar("SELECT default_locale FROM merchant.stores WHERE merchant_account_id=$1 AND id=$2 FOR UPDATE").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).fetch_optional(&mut **tx).await.map_err(database_error)?.ok_or_else(||store_not_found(store))
}
async fn require_store(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
) -> Result<(), ApplicationError> {
    lock_default_locale(tx, actor, store).await.map(|_| ())
}
async fn require_translatable_locale(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    locale: &Locale,
) -> Result<(), ApplicationError> {
    let default = lock_default_locale(tx, actor, store).await?;
    if default == locale.as_str() {
        return Err(ApplicationError::Conflict {
            code: "default_locale_uses_canonical_content",
            message: "the default Locale uses canonical Catalog content",
        });
    }
    let enabled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM merchant.store_locales WHERE merchant_account_id=$1 AND store_id=$2 AND locale=$3)").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(locale.as_str()).fetch_one(&mut **tx).await.map_err(database_error)?;
    if enabled {
        Ok(())
    } else {
        Err(ApplicationError::Conflict {
            code: "locale_not_enabled",
            message: "the Locale is not enabled for this Store",
        })
    }
}
async fn require_product(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    id: ProductId,
) -> Result<(), ApplicationError> {
    require_status(tx, actor, store, "products", id.as_uuid(), "product").await
}
async fn require_collection(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    id: CollectionId,
) -> Result<(), ApplicationError> {
    require_status(tx, actor, store, "collections", id.as_uuid(), "collection").await
}
async fn require_status(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    table: &str,
    id: Uuid,
    resource: &'static str,
) -> Result<(), ApplicationError> {
    let query = match table {
        "products" => {
            "SELECT status::text FROM catalog.products WHERE merchant_account_id=$1 AND store_id=$2 AND id=$3"
        }
        "collections" => {
            "SELECT status::text FROM catalog.collections WHERE merchant_account_id=$1 AND store_id=$2 AND id=$3"
        }
        _ => unreachable!(),
    };
    let status: Option<String> = sqlx::query_scalar(query)
        .bind(actor.merchant_account_id().as_uuid())
        .bind(store.as_uuid())
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(database_error)?;
    match status.as_deref() {
        None => Err(ApplicationError::NotFound {
            resource,
            id: id.to_string(),
        }),
        Some("archived") => Err(ApplicationError::Conflict {
            code: "resource_archived",
            message: "an archived resource cannot be translated",
        }),
        Some(_) => Ok(()),
    }
}
async fn require_media(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    media: MediaAssetId,
) -> Result<(), ApplicationError> {
    let status:Option<String>=sqlx::query_scalar("SELECT status::text FROM catalog.media_assets WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 AND id=$4").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(product.as_uuid()).bind(media.as_uuid()).fetch_optional(&mut **tx).await.map_err(database_error)?;
    match status.as_deref() {
        None => Err(ApplicationError::NotFound {
            resource: "media_asset",
            id: media.as_uuid().to_string(),
        }),
        Some("archived") => Err(ApplicationError::Conflict {
            code: "media_archived",
            message: "an archived Media Asset cannot be translated",
        }),
        Some(_) => Ok(()),
    }
}
async fn require_variants(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    variants: &[ProductVariantTranslation],
) -> Result<(), ApplicationError> {
    if variants.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = variants
        .iter()
        .map(|value| value.product_variant_id.as_uuid())
        .collect();
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM catalog.product_variants WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 AND id=ANY($4)").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(product.as_uuid()).bind(&ids).fetch_one(&mut **tx).await.map_err(database_error)?;
    if usize::try_from(count).ok() == Some(ids.len()) {
        Ok(())
    } else {
        Err(ApplicationError::Validation {
            violations: vec![chaos_domain::FieldViolation {
                field: "variants",
                reason: "must reference Product Variants in the translated Product".into(),
            }],
        })
    }
}
async fn load_product_translation(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    locale: &Locale,
) -> Result<Option<ProductTranslation>, ApplicationError> {
    let row:Option<(String,String,OffsetDateTime,OffsetDateTime)>=sqlx::query_as("SELECT title,description,created_at,updated_at FROM catalog.product_translations WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 AND locale=$4").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(product.as_uuid()).bind(locale.as_str()).fetch_optional(&mut **tx).await.map_err(database_error)?;
    let Some(row) = row else { return Ok(None) };
    let variants:Vec<(Uuid,String)>=sqlx::query_as("SELECT product_variant_id,title FROM catalog.product_variant_translations WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 AND locale=$4 ORDER BY product_variant_id").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(product.as_uuid()).bind(locale.as_str()).fetch_all(&mut **tx).await.map_err(database_error)?;
    Ok(Some(ProductTranslation {
        content: LocalizedContent::new(locale.clone(), row.0, row.1)?,
        variants: variants
            .into_iter()
            .map(|value| {
                Ok(ProductVariantTranslation {
                    product_variant_id: ProductVariantId::from_uuid(value.0),
                    title: LocalizedTitle::new(value.1)?,
                })
            })
            .collect::<Result<_, ApplicationError>>()?,
        created_at: row.2,
        updated_at: row.3,
    }))
}
async fn replay_product_translation(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    locale: &Locale,
) -> Result<ProductTranslation, ApplicationError> {
    load_product_translation(tx, actor, store, product, locale)
        .await?
        .ok_or_else(invalid_snapshot)
}
async fn load_collection_translation(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    collection: CollectionId,
    locale: &Locale,
) -> Result<Option<CollectionTranslation>, ApplicationError> {
    let row:Option<(String,String,OffsetDateTime,OffsetDateTime)>=sqlx::query_as("SELECT title,description,created_at,updated_at FROM catalog.collection_translations WHERE merchant_account_id=$1 AND store_id=$2 AND collection_id=$3 AND locale=$4").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(collection.as_uuid()).bind(locale.as_str()).fetch_optional(&mut **tx).await.map_err(database_error)?;
    row.map(|value| {
        Ok(CollectionTranslation {
            content: LocalizedContent::new(locale.clone(), value.0, value.1)?,
            created_at: value.2,
            updated_at: value.3,
        })
    })
    .transpose()
}
async fn replay_collection_translation(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    collection: CollectionId,
    locale: &Locale,
) -> Result<CollectionTranslation, ApplicationError> {
    load_collection_translation(tx, actor, store, collection, locale)
        .await?
        .ok_or_else(invalid_snapshot)
}
async fn load_media_translation(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    media: MediaAssetId,
    locale: &Locale,
) -> Result<Option<MediaTranslation>, ApplicationError> {
    let row:Option<(String,OffsetDateTime,OffsetDateTime)>=sqlx::query_as("SELECT alt_text,created_at,updated_at FROM catalog.media_asset_translations WHERE merchant_account_id=$1 AND store_id=$2 AND product_id=$3 AND media_asset_id=$4 AND locale=$5").bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(product.as_uuid()).bind(media.as_uuid()).bind(locale.as_str()).fetch_optional(&mut **tx).await.map_err(database_error)?;
    row.map(|value| {
        Ok(MediaTranslation {
            locale: locale.clone(),
            alt_text: LocalizedAltText::new(value.0)?,
            created_at: value.1,
            updated_at: value.2,
        })
    })
    .transpose()
}
async fn replay_media_translation(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    media: MediaAssetId,
    locale: &Locale,
) -> Result<MediaTranslation, ApplicationError> {
    load_media_translation(tx, actor, store, product, media, locale)
        .await?
        .ok_or_else(invalid_snapshot)
}

async fn locale_event(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    locale: &Locale,
    previous: Option<&str>,
    kind: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query("INSERT INTO merchant.store_locale_events (id,merchant_account_id,store_id,locale,previous_locale,event_kind,actor_user_id,occurred_at) VALUES ($1,$2,$3,$4,$5,$6::merchant.store_locale_event_kind,$7,$8)").bind(Uuid::now_v7()).bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(locale.as_str()).bind(previous).bind(kind).bind(actor.audit_user_id().as_uuid()).bind(now).execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}
async fn product_event(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    locale: &Locale,
    kind: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query("INSERT INTO catalog.product_translation_events (id,merchant_account_id,store_id,product_id,locale,event_kind,actor_user_id,occurred_at) VALUES ($1,$2,$3,$4,$5,$6::catalog.translation_event_kind,$7,$8)").bind(Uuid::now_v7()).bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(product.as_uuid()).bind(locale.as_str()).bind(kind).bind(actor.audit_user_id().as_uuid()).bind(now).execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}
async fn collection_event(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    collection: CollectionId,
    locale: &Locale,
    kind: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query("INSERT INTO catalog.collection_translation_events (id,merchant_account_id,store_id,collection_id,locale,event_kind,actor_user_id,occurred_at) VALUES ($1,$2,$3,$4,$5,$6::catalog.translation_event_kind,$7,$8)").bind(Uuid::now_v7()).bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(collection.as_uuid()).bind(locale.as_str()).bind(kind).bind(actor.audit_user_id().as_uuid()).bind(now).execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}
#[allow(clippy::too_many_arguments)]
async fn media_event(
    tx: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
    store: StoreId,
    product: ProductId,
    media: MediaAssetId,
    locale: &Locale,
    kind: &str,
    now: OffsetDateTime,
) -> Result<(), ApplicationError> {
    sqlx::query("INSERT INTO catalog.media_translation_events (id,merchant_account_id,store_id,product_id,media_asset_id,locale,event_kind,actor_user_id,occurred_at) VALUES ($1,$2,$3,$4,$5,$6,$7::catalog.translation_event_kind,$8,$9)").bind(Uuid::now_v7()).bind(actor.merchant_account_id().as_uuid()).bind(store.as_uuid()).bind(product.as_uuid()).bind(media.as_uuid()).bind(locale.as_str()).bind(kind).bind(actor.audit_user_id().as_uuid()).bind(now).execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}
async fn reserve(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<bool, ApplicationError> {
    Ok(idempotency::reserve(
        tx,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
    )
    .await?
    .is_some())
}
async fn complete(
    tx: &mut Transaction<'static, Postgres>,
    actor: &AdminActor,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        tx,
        &IdempotencyScope::MerchantAccount(actor.merchant_account_id().as_uuid()),
        operation,
        request,
        200,
        json!({"completed":true}),
    )
    .await
}
fn parse_locale(value: &str) -> Result<Locale, ApplicationError> {
    Locale::parse(value).map_err(Into::into)
}
fn store_not_found(id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: id.as_uuid().to_string(),
    }
}
fn invalid_snapshot() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "the Localization persistence snapshot is invalid"
    ))
}
fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
