use std::collections::HashMap;

use crate::{
    ApplicationError,
    contracts::{
        MachineActor, StorefrontCatalogProduct, StorefrontCatalogRepository,
        StorefrontCatalogVariant, StorefrontMediaAsset, StorefrontProductCollection,
        StorefrontProductOption, StorefrontProductOptionValue, StorefrontRatingSummary,
        StorefrontSelectedOption,
    },
    error::database_error,
};
use async_trait::async_trait;
use chaos_domain::{
    CurrencyCode,
    catalog::{
        CollectionId, MediaAssetId, MediaKind, ProductId, ProductOptionId, ProductOptionValueId,
        ProductVariantId,
    },
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct ProductRow {
    id: Uuid,
    handle: String,
    title: String,
    description: String,
    metadata: Option<serde_json::Value>,
}

#[derive(sqlx::FromRow)]
struct VariantRow {
    product_id: Uuid,
    id: Uuid,
    title: String,
    sku: Option<String>,
    track_inventory: bool,
    available_quantity: i64,
    amount_minor: i64,
    currency: String,
    metadata: Option<serde_json::Value>,
}

#[derive(sqlx::FromRow)]
struct MediaRow {
    product_id: Uuid,
    id: Uuid,
    scope: String,
    option_id: Option<Uuid>,
    option_value_id: Option<Uuid>,
    product_variant_id: Option<Uuid>,
    media_type: String,
    kind: String,
    alt_text: String,
    position: i16,
    url: String,
}

#[derive(sqlx::FromRow)]
struct MetadataAttachmentRow {
    product_id: Uuid,
    meta_path: String,
    asset_id: Uuid,
    media_type: String,
    public_url: Option<String>,
}

#[derive(Clone)]
pub struct PostgresStorefrontCatalogRepository {
    pool: PgPool,
}

impl PostgresStorefrontCatalogRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        crate::adapters::postgres::database::set_store_context(&mut transaction, actor.store_id)
            .await
            .map_err(database_error)?;
        Ok(transaction)
    }

    async fn variants(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_id: ProductId,
        currency: Option<CurrencyCode>,
    ) -> Result<Vec<StorefrontCatalogVariant>, ApplicationError> {
        let rows = sqlx::query_as::<_, VariantRow>(
            "WITH selected_price_list AS ( \
                 SELECT selected.id, selected.currency::text \
                 FROM chaos_commerce.stores AS store \
                 CROSS JOIN LATERAL chaos_commerce.resolve_price_list( \
                     store.id, COALESCE($3::char(3), store.currency), CURRENT_TIMESTAMP \
                 ) AS selected \
                 WHERE store.id = $1 \
             ) \
            SELECT variant.product_id, variant.id, variant.title, variant.sku::text AS sku, \
                    variant.track_inventory, \
                    variant.on_hand_quantity - variant.reserved_quantity AS available_quantity, \
                    price.amount_minor, selected.currency, variant.meta AS metadata \
             FROM chaos_commerce.product_variants AS variant \
             INNER JOIN selected_price_list AS selected ON true \
             INNER JOIN chaos_commerce.price_list_items AS price \
               ON price.store_id = variant.store_id \
              AND price.price_list_id = selected.id \
              AND price.product_variant_id = variant.id \
             WHERE variant.store_id = $1 \
               AND variant.product_id = $2 \
               AND variant.status = 'active' \
             ORDER BY variant.id ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .bind(currency.map(|value| value.as_str().to_owned()))
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;

        let mut selections = variant_selected_options(transaction, actor, product_id).await?;
        rows.into_iter()
            .map(|row| {
                let selected_options = selections.remove(&row.id).unwrap_or_default();
                catalog_variant(row, selected_options)
            })
            .collect()
    }

    async fn options(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_id: ProductId,
    ) -> Result<Vec<StorefrontProductOption>, ApplicationError> {
        let option_rows = sqlx::query_as::<_, (Uuid, String, i16)>(
            "SELECT id, name::text, position \
             FROM chaos_commerce.product_options \
             WHERE store_id = $1 AND product_id = $2 \
               AND archived_at IS NULL \
             ORDER BY position ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let value_rows = sqlx::query_as::<_, (Uuid, Uuid, String, i16)>(
            "SELECT id, option_id, value::text, position \
             FROM chaos_commerce.product_option_values \
             WHERE store_id = $1 AND product_id = $2 \
               AND archived_at IS NULL \
             ORDER BY option_id ASC, position ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;

        let mut options = option_rows
            .into_iter()
            .map(|(id, name, position)| {
                Ok(StorefrontProductOption {
                    id: ProductOptionId::from_uuid(id),
                    name,
                    position: u16::try_from(position).map_err(|_| {
                        ApplicationError::Unexpected(anyhow::anyhow!(
                            "database contains a negative Catalog position"
                        ))
                    })?,
                    values: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        for (id, option_id, value, position) in value_rows {
            let option = options
                .iter_mut()
                .find(|option| option.id.as_uuid() == option_id)
                .ok_or_else(|| {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "database contains an option value with no parent option"
                    ))
                })?;
            option.values.push(StorefrontProductOptionValue {
                id: ProductOptionValueId::from_uuid(id),
                value,
                position: u16::try_from(position).map_err(|_| {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "database contains a negative Catalog position"
                    ))
                })?,
            });
        }
        Ok(options)
    }

    async fn media(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_id: ProductId,
    ) -> Result<Vec<StorefrontMediaAsset>, ApplicationError> {
        let rows = sqlx::query_as::<_, MediaRow>(
            "SELECT link.product_id, media.id, 'product'::text AS scope, \
                    NULL::uuid AS option_id, NULL::uuid AS option_value_id, \
                    NULL::uuid AS product_variant_id, media.media_type, \
                    media.media_kind::text AS kind, link.alt_text, link.position, \
                    media.public_url AS url \
             FROM chaos_commerce.product_media_assets AS link \
             INNER JOIN chaos_commerce.media_assets AS media \
                ON media.store_id=link.store_id AND media.id=link.media_asset_id \
             WHERE link.store_id=$1 AND link.product_id=$2 \
               AND link.archived_at IS NULL AND media.status='ready' \
             UNION ALL \
             SELECT link.product_id, media.id, 'option_value'::text, link.option_id, \
                    link.option_value_id, NULL::uuid, \
                    media.media_type,media.media_kind::text,link.alt_text,link.position,media.public_url \
             FROM chaos_commerce.product_option_value_media_assets AS link \
             INNER JOIN chaos_commerce.media_assets AS media \
                ON media.store_id=link.store_id AND media.id=link.media_asset_id \
             INNER JOIN chaos_commerce.product_options AS option \
                ON option.store_id=link.store_id AND option.product_id=link.product_id \
               AND option.id=link.option_id AND option.archived_at IS NULL \
             INNER JOIN chaos_commerce.product_option_values AS option_value \
                ON option_value.store_id=link.store_id AND option_value.product_id=link.product_id \
               AND option_value.option_id=link.option_id AND option_value.id=link.option_value_id \
               AND option_value.archived_at IS NULL \
             WHERE link.store_id=$1 AND link.product_id=$2 \
               AND link.archived_at IS NULL AND media.status='ready' \
             UNION ALL \
             SELECT link.product_id, media.id, 'variant'::text, NULL::uuid, NULL::uuid, \
                    link.product_variant_id, \
                    media.media_type,media.media_kind::text,link.alt_text,link.position,media.public_url \
             FROM chaos_commerce.product_variant_media_assets AS link \
             INNER JOIN chaos_commerce.media_assets AS media \
                ON media.store_id=link.store_id AND media.id=link.media_asset_id \
             INNER JOIN chaos_commerce.product_variants AS variant \
                ON variant.store_id=link.store_id AND variant.product_id=link.product_id \
               AND variant.id=link.product_variant_id AND variant.status='active' \
             WHERE link.store_id=$1 AND link.product_id=$2 \
               AND link.archived_at IS NULL AND media.status='ready' \
             ORDER BY 10, 3, 2",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        rows.into_iter().map(media_asset).collect()
    }

    async fn rating(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_id: ProductId,
    ) -> Result<Option<StorefrontRatingSummary>, ApplicationError> {
        let row = sqlx::query_as::<_, (f64, i64)>(
            "SELECT ROUND(AVG(rating)::numeric, 1)::float8 AS average, COUNT(*) AS count \
             FROM chaos_commerce.reviews \
             WHERE store_id = $1 \
               AND product_id = $2 \
               AND status = 'approved' \
               AND parent_review_id IS NULL \
             HAVING COUNT(*) > 0",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
        Ok(row.map(|(average, count)| StorefrontRatingSummary { average, count }))
    }

    async fn metadata(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_id: ProductId,
        metadata: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ApplicationError> {
        let rows = sqlx::query_as::<_, MetadataAttachmentRow>(
            "SELECT link.product_id, link.meta_path, media.id AS asset_id, \
                    media.media_type, media.public_url \
             FROM chaos_commerce.product_meta_media_assets AS link \
             INNER JOIN chaos_commerce.media_assets AS media \
                ON media.store_id=link.store_id AND media.id=link.media_asset_id \
             WHERE link.store_id=$1 AND link.product_id=$2 \
               AND link.archived_at IS NULL AND media.status='ready' \
             ORDER BY link.meta_path, media.id",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        if rows.is_empty() {
            return Ok(metadata);
        }
        let metadata = metadata.ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "Product metadata Media attachment has no Product metadata object"
            ))
        })?;
        Ok(Some(apply_metadata_attachments(metadata, rows)?))
    }

    async fn collections(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_id: ProductId,
    ) -> Result<Vec<StorefrontProductCollection>, ApplicationError> {
        let rows = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT collection.id, collection.handle::text, collection.title \
             FROM chaos_commerce.collection_products AS member \
             INNER JOIN chaos_commerce.collections AS collection \
               ON collection.store_id = member.store_id \
              AND collection.id = member.collection_id \
             INNER JOIN chaos_commerce.collection_publications AS publication \
               ON publication.store_id = collection.store_id \
              AND publication.collection_id = collection.id \
              AND publication.channel_id = $2 \
             WHERE member.store_id = $1 \
               AND member.product_id = $3 \
               AND collection.status = 'active' \
             ORDER BY collection.handle ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.channel_id.map(|id| id.as_uuid()))
        .bind(product_id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        Ok(rows
            .into_iter()
            .map(|(id, handle, title)| StorefrontProductCollection {
                id: CollectionId::from_uuid(id),
                handle,
                title,
            })
            .collect())
    }

    async fn variants_for_products(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_ids: &[Uuid],
        currency: Option<CurrencyCode>,
    ) -> Result<HashMap<Uuid, Vec<StorefrontCatalogVariant>>, ApplicationError> {
        if product_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, VariantRow>(
            "WITH selected_price_list AS ( \
                 SELECT selected.id, selected.currency::text \
                 FROM chaos_commerce.stores AS store \
                 CROSS JOIN LATERAL chaos_commerce.resolve_price_list( \
                     store.id, COALESCE($3::char(3), store.currency), CURRENT_TIMESTAMP \
                 ) AS selected \
                 WHERE store.id = $1 \
             ) \
            SELECT variant.product_id, variant.id, variant.title, variant.sku::text AS sku, \
                    variant.track_inventory, \
                    variant.on_hand_quantity - variant.reserved_quantity AS available_quantity, \
                    price.amount_minor, selected.currency, variant.meta AS metadata \
             FROM chaos_commerce.product_variants AS variant \
             INNER JOIN selected_price_list AS selected ON true \
             INNER JOIN chaos_commerce.price_list_items AS price \
               ON price.store_id = variant.store_id \
              AND price.price_list_id = selected.id \
              AND price.product_variant_id = variant.id \
             WHERE variant.store_id = $1 \
               AND variant.product_id = ANY($2::uuid[]) \
               AND variant.status = 'active' \
             ORDER BY variant.product_id ASC, variant.id ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_ids)
        .bind(currency.map(|value| value.as_str().to_owned()))
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;

        let mut selections =
            variant_selected_options_for_products(transaction, actor, product_ids).await?;
        let mut variants_by_product: HashMap<Uuid, Vec<StorefrontCatalogVariant>> = HashMap::new();
        for row in rows {
            let product_id = row.product_id;
            let selected_options = selections.remove(&(product_id, row.id)).unwrap_or_default();
            let variant = catalog_variant(row, selected_options)?;
            variants_by_product
                .entry(product_id)
                .or_default()
                .push(variant);
        }
        Ok(variants_by_product)
    }

    async fn options_for_products(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<StorefrontProductOption>>, ApplicationError> {
        if product_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let option_rows = sqlx::query_as::<_, (Uuid, Uuid, String, i16)>(
            "SELECT product_id, id, name::text, position \
             FROM chaos_commerce.product_options \
             WHERE store_id = $1 AND product_id = ANY($2::uuid[]) \
               AND archived_at IS NULL \
             ORDER BY product_id, position ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let value_rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, i16)>(
            "SELECT product_id, id, option_id, value::text, position \
             FROM chaos_commerce.product_option_values \
             WHERE store_id = $1 AND product_id = ANY($2::uuid[]) \
               AND archived_at IS NULL \
             ORDER BY product_id, option_id ASC, position ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;

        let mut options_by_product: HashMap<Uuid, Vec<StorefrontProductOption>> = HashMap::new();
        let mut option_indexes: HashMap<(Uuid, Uuid), usize> = HashMap::new();
        for (product_id, id, name, position) in option_rows {
            let options = options_by_product.entry(product_id).or_default();
            let index = options.len();
            options.push(StorefrontProductOption {
                id: ProductOptionId::from_uuid(id),
                name,
                position: u16::try_from(position).map_err(|_| {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "database contains a negative Catalog position"
                    ))
                })?,
                values: Vec::new(),
            });
            option_indexes.insert((product_id, id), index);
        }
        for (product_id, id, option_id, value, position) in value_rows {
            let index = option_indexes
                .get(&(product_id, option_id))
                .copied()
                .ok_or_else(|| {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "database contains an option value with no parent option"
                    ))
                })?;
            options_by_product
                .get_mut(&product_id)
                .and_then(|options| options.get_mut(index))
                .ok_or_else(|| {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "database contains an option value with no parent option"
                    ))
                })?
                .values
                .push(StorefrontProductOptionValue {
                    id: ProductOptionValueId::from_uuid(id),
                    value,
                    position: u16::try_from(position).map_err(|_| {
                        ApplicationError::Unexpected(anyhow::anyhow!(
                            "database contains a negative Catalog position"
                        ))
                    })?,
                });
        }
        Ok(options_by_product)
    }

    async fn media_for_products(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<StorefrontMediaAsset>>, ApplicationError> {
        if product_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, MediaRow>(
            "SELECT link.product_id, media.id, 'product'::text AS scope, \
                    NULL::uuid AS option_id, NULL::uuid AS option_value_id, \
                    NULL::uuid AS product_variant_id, media.media_type, \
                    media.media_kind::text AS kind, link.alt_text, link.position, \
                    media.public_url AS url \
             FROM chaos_commerce.product_media_assets AS link \
             INNER JOIN chaos_commerce.media_assets AS media \
                ON media.store_id = link.store_id AND media.id = link.media_asset_id \
             WHERE link.store_id = $1 AND link.product_id = ANY($2::uuid[]) \
               AND link.archived_at IS NULL AND media.status = 'ready' \
             UNION ALL \
             SELECT link.product_id, media.id, 'option_value'::text, link.option_id, link.option_value_id, NULL::uuid, \
                    media.media_type, media.media_kind::text, link.alt_text, link.position, media.public_url \
             FROM chaos_commerce.product_option_value_media_assets AS link \
             INNER JOIN chaos_commerce.media_assets AS media \
                ON media.store_id = link.store_id AND media.id = link.media_asset_id \
             INNER JOIN chaos_commerce.product_options AS option \
                ON option.store_id=link.store_id AND option.product_id=link.product_id \
               AND option.id=link.option_id AND option.archived_at IS NULL \
             INNER JOIN chaos_commerce.product_option_values AS option_value \
                ON option_value.store_id=link.store_id AND option_value.product_id=link.product_id \
               AND option_value.option_id=link.option_id AND option_value.id=link.option_value_id \
               AND option_value.archived_at IS NULL \
             WHERE link.store_id = $1 AND link.product_id = ANY($2::uuid[]) \
               AND link.archived_at IS NULL AND media.status = 'ready' \
             UNION ALL \
             SELECT link.product_id, media.id, 'variant'::text, NULL::uuid, NULL::uuid, link.product_variant_id, \
                    media.media_type, media.media_kind::text, link.alt_text, link.position, media.public_url \
             FROM chaos_commerce.product_variant_media_assets AS link \
             INNER JOIN chaos_commerce.media_assets AS media \
                ON media.store_id = link.store_id AND media.id = link.media_asset_id \
             INNER JOIN chaos_commerce.product_variants AS variant \
                ON variant.store_id=link.store_id AND variant.product_id=link.product_id \
               AND variant.id=link.product_variant_id AND variant.status='active' \
             WHERE link.store_id = $1 AND link.product_id = ANY($2::uuid[]) \
               AND link.archived_at IS NULL AND media.status = 'ready' \
             ORDER BY 1, 10, 3, 2",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let mut media_by_product: HashMap<Uuid, Vec<StorefrontMediaAsset>> = HashMap::new();
        for row in rows {
            let product_id = row.product_id;
            media_by_product
                .entry(product_id)
                .or_default()
                .push(media_asset(row)?);
        }
        Ok(media_by_product)
    }

    async fn collections_for_products(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<StorefrontProductCollection>>, ApplicationError> {
        if product_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
            "SELECT member.product_id, collection.id, collection.handle::text, collection.title \
             FROM chaos_commerce.collection_products AS member \
             INNER JOIN chaos_commerce.collections AS collection \
               ON collection.store_id = member.store_id \
              AND collection.id = member.collection_id \
             INNER JOIN chaos_commerce.collection_publications AS publication \
               ON publication.store_id = collection.store_id \
              AND publication.collection_id = collection.id \
              AND publication.channel_id = $2 \
             WHERE member.store_id = $1 \
               AND member.product_id = ANY($3::uuid[]) \
               AND collection.status = 'active' \
             ORDER BY member.product_id, collection.handle ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.channel_id.map(|id| id.as_uuid()))
        .bind(product_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let mut collections_by_product: HashMap<Uuid, Vec<StorefrontProductCollection>> =
            HashMap::new();
        for (product_id, id, handle, title) in rows {
            collections_by_product.entry(product_id).or_default().push(
                StorefrontProductCollection {
                    id: CollectionId::from_uuid(id),
                    handle,
                    title,
                },
            );
        }
        Ok(collections_by_product)
    }

    /// Approved, top-level review rating per Product, for a batch listing.
    /// The `reviews_rating_shape_check` constraint guarantees every row this
    /// query matches (non-reply) has a non-null rating between 1 and 5.
    async fn rating_for_products(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, StorefrontRatingSummary>, ApplicationError> {
        if product_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, (Uuid, f64, i64)>(
            "SELECT product_id, ROUND(AVG(rating)::numeric, 1)::float8 AS average, \
                    COUNT(*) AS count \
             FROM chaos_commerce.reviews \
             WHERE store_id = $1 \
               AND product_id = ANY($2::uuid[]) \
               AND status = 'approved' \
               AND parent_review_id IS NULL \
             GROUP BY product_id",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        Ok(rows
            .into_iter()
            .map(|(product_id, average, count)| {
                (product_id, StorefrontRatingSummary { average, count })
            })
            .collect())
    }

    async fn metadata_for_products(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        products: &[(Uuid, Option<serde_json::Value>)],
    ) -> Result<HashMap<Uuid, Option<serde_json::Value>>, ApplicationError> {
        if products.is_empty() {
            return Ok(HashMap::new());
        }
        let product_ids: Vec<Uuid> = products.iter().map(|(id, _)| *id).collect();
        let rows = sqlx::query_as::<_, MetadataAttachmentRow>(
            "SELECT link.product_id, link.meta_path, media.id AS asset_id, \
                    media.media_type, media.public_url \
             FROM chaos_commerce.product_meta_media_assets AS link \
             INNER JOIN chaos_commerce.media_assets AS media \
                ON media.store_id = link.store_id AND media.id = link.media_asset_id \
             WHERE link.store_id = $1 AND link.product_id = ANY($2::uuid[]) \
               AND link.archived_at IS NULL AND media.status = 'ready' \
             ORDER BY link.product_id, link.meta_path, media.id",
        )
        .bind(actor.store_id.as_uuid())
        .bind(&product_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let mut metadata_by_product: HashMap<Uuid, Option<serde_json::Value>> = products
            .iter()
            .map(|(id, metadata)| (*id, metadata.clone()))
            .collect();
        let mut attachments_by_product: HashMap<Uuid, Vec<MetadataAttachmentRow>> = HashMap::new();
        for row in rows {
            attachments_by_product
                .entry(row.product_id)
                .or_default()
                .push(row);
        }
        for (product_id, attachments) in attachments_by_product {
            let metadata = metadata_by_product
                .get_mut(&product_id)
                .ok_or_else(|| {
                    ApplicationError::Unexpected(anyhow::anyhow!("missing Product metadata row"))
                })?
                .take()
                .ok_or_else(|| {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "Product metadata Media attachment has no Product metadata object"
                    ))
                })?;
            metadata_by_product.insert(
                product_id,
                Some(apply_metadata_attachments(metadata, attachments)?),
            );
        }
        Ok(metadata_by_product)
    }
}

#[async_trait]
impl StorefrontCatalogRepository for PostgresStorefrontCatalogRepository {
    async fn list_products(
        &self,
        actor: &MachineActor,
        currency: Option<CurrencyCode>,
        query: Option<&str>,
        collection_handle: Option<&str>,
        after: Option<ProductId>,
        limit: u16,
    ) -> Result<Vec<StorefrontCatalogProduct>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let mut scan_after = after;
        let mut products = Vec::with_capacity(usize::from(limit));
        while products.len() < usize::from(limit) {
            let rows = sqlx::query_as::<_, ProductRow>(
                "WITH selected_collection AS ( \
                     SELECT collection.id \
                     FROM chaos_commerce.collections AS collection \
                     INNER JOIN chaos_commerce.collection_publications AS publication \
                       ON publication.store_id = collection.store_id \
                      AND publication.collection_id = collection.id \
                      AND publication.channel_id = $2 \
                     WHERE collection.store_id = $1 \
                       AND collection.handle = $5 \
                       AND collection.status = 'active' \
                     LIMIT 1 \
                 ), collection_members AS ( \
                     SELECT member.product_id, member.position \
                     FROM chaos_commerce.collection_products AS member \
                     INNER JOIN selected_collection AS selected \
                       ON selected.id = member.collection_id \
                     WHERE member.store_id = $1 \
                 ) \
                SELECT product.id, product.handle::text AS handle, product.title, \
                        product.description, product.meta AS metadata \
                 FROM chaos_commerce.products AS product \
                 INNER JOIN chaos_commerce.stores AS store \
                   ON store.id = product.store_id \
                 INNER JOIN chaos_commerce.channels AS channel \
                   ON channel.store_id = product.store_id \
                  AND channel.id = $2 \
                 INNER JOIN chaos_commerce.product_publications AS publication \
                   ON publication.store_id = product.store_id \
                  AND publication.product_id = product.id \
                  AND publication.channel_id = channel.id \
                 LEFT JOIN chaos_commerce.product_documents AS search_document \
                   ON search_document.store_id = product.store_id \
                  AND search_document.product_id = product.id \
                 LEFT JOIN collection_members AS member \
                   ON member.product_id = product.id \
                 WHERE product.store_id = $1 \
                   AND store.status = 'active' \
                   AND channel.status = 'active' \
                   AND product.status = 'active' \
                   AND ($4::text IS NULL OR search_document.document @@ websearch_to_tsquery('simple', $4)) \
                   AND ($5::text IS NULL OR member.product_id IS NOT NULL) \
                   AND ($5::text IS NULL OR $3::uuid IS NULL OR member.position > ( \
                       SELECT anchor.position FROM collection_members AS anchor WHERE anchor.product_id = $3 \
                   )) \
                   AND (($5::text IS NULL AND ($3::uuid IS NULL OR product.id > $3)) OR $5::text IS NOT NULL) \
                 ORDER BY CASE WHEN $5::text IS NOT NULL THEN member.position END ASC NULLS LAST, \
                          CASE WHEN $5::text IS NULL THEN product.id END ASC \
                 LIMIT 100",
            )
            .bind(actor.store_id.as_uuid())
            .bind(actor.channel_id.map(|id| id.as_uuid()))
            .bind(scan_after.map(ProductId::as_uuid))
            .bind(query)
            .bind(collection_handle)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
            if rows.is_empty() {
                break;
            }
            let rows_len = rows.len();
            let product_ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
            let mut variants_by_product =
                Self::variants_for_products(&mut transaction, actor, &product_ids, currency)
                    .await?;
            let display_product_ids: Vec<Uuid> = product_ids
                .iter()
                .copied()
                .filter(|product_id| {
                    variants_by_product
                        .get(product_id)
                        .is_some_and(|variants| !variants.is_empty())
                })
                .collect();
            let mut options_by_product =
                Self::options_for_products(&mut transaction, actor, &display_product_ids).await?;
            let mut media_by_product =
                Self::media_for_products(&mut transaction, actor, &display_product_ids).await?;
            let mut collections_by_product =
                Self::collections_for_products(&mut transaction, actor, &display_product_ids)
                    .await?;
            let mut rating_by_product =
                Self::rating_for_products(&mut transaction, actor, &display_product_ids).await?;
            let metadata_inputs: Vec<(Uuid, Option<serde_json::Value>)> = rows
                .iter()
                .filter(|row| display_product_ids.contains(&row.id))
                .map(|row| (row.id, row.metadata.clone()))
                .collect();
            let mut metadata_by_product =
                Self::metadata_for_products(&mut transaction, actor, &metadata_inputs).await?;
            for row in rows {
                let id = ProductId::from_uuid(row.id);
                scan_after = Some(id);
                if let Some(variants) = variants_by_product.remove(&id.as_uuid()) {
                    if variants.is_empty() {
                        continue;
                    }
                    let options = options_by_product.remove(&id.as_uuid()).unwrap_or_default();
                    let media = media_by_product.remove(&id.as_uuid()).unwrap_or_default();
                    let collections = collections_by_product
                        .remove(&id.as_uuid())
                        .unwrap_or_default();
                    let metadata = metadata_by_product
                        .remove(&id.as_uuid())
                        .flatten()
                        .or(row.metadata);
                    let rating = rating_by_product.remove(&id.as_uuid());
                    products.push(StorefrontCatalogProduct {
                        id,
                        handle: row.handle,
                        title: row.title,
                        description: row.description,
                        options,
                        variants,
                        media,
                        collections,
                        metadata,
                        rating,
                    });
                    if products.len() == usize::from(limit) {
                        break;
                    }
                }
            }
            if rows_len < 100 {
                break;
            }
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(products)
    }

    async fn get_product_by_handle(
        &self,
        actor: &MachineActor,
        currency: Option<CurrencyCode>,
        handle: &str,
    ) -> Result<Option<StorefrontCatalogProduct>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let row = sqlx::query_as::<_, ProductRow>(
            "SELECT product.id, product.handle::text AS handle, product.title, \
                    product.description, product.meta AS metadata \
             FROM chaos_commerce.products AS product \
             INNER JOIN chaos_commerce.stores AS store \
               ON store.id = product.store_id \
             INNER JOIN chaos_commerce.channels AS channel \
               ON channel.store_id = product.store_id \
              AND channel.id = $2 \
             INNER JOIN chaos_commerce.product_publications AS publication \
               ON publication.store_id = product.store_id \
              AND publication.product_id = product.id \
              AND publication.channel_id = channel.id \
             WHERE product.store_id = $1 \
               AND product.handle = $3 \
               AND store.status = 'active' \
               AND channel.status = 'active' \
               AND product.status = 'active'",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.channel_id.map(|id| id.as_uuid()))
        .bind(handle)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let Some(row) = row else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let id = ProductId::from_uuid(row.id);
        let variants = Self::variants(&mut transaction, actor, id, currency).await?;
        let options = Self::options(&mut transaction, actor, id).await?;
        let media = Self::media(&mut transaction, actor, id).await?;
        let collections = Self::collections(&mut transaction, actor, id).await?;
        let metadata = Self::metadata(&mut transaction, actor, id, row.metadata).await?;
        let rating = Self::rating(&mut transaction, actor, id).await?;
        transaction.commit().await.map_err(database_error)?;
        if variants.is_empty() {
            return Ok(None);
        }
        Ok(Some(StorefrontCatalogProduct {
            id,
            handle: row.handle,
            title: row.title,
            description: row.description,
            options,
            variants,
            media,
            collections,
            metadata,
            rating,
        }))
    }
}

fn catalog_variant(
    row: VariantRow,
    selected_options: Vec<StorefrontSelectedOption>,
) -> Result<StorefrontCatalogVariant, ApplicationError> {
    Ok(StorefrontCatalogVariant {
        id: ProductVariantId::from_uuid(row.id),
        title: row.title,
        sku: row.sku,
        track_inventory: row.track_inventory,
        available_quantity: row.available_quantity,
        amount_minor: row.amount_minor,
        currency: CurrencyCode::parse(&row.currency).map_err(|_| {
            ApplicationError::Unexpected(anyhow::anyhow!("database contains an invalid currency"))
        })?,
        selected_options,
        metadata: row.metadata,
    })
}

fn media_asset(row: MediaRow) -> Result<StorefrontMediaAsset, ApplicationError> {
    let scope = match row.scope.as_str() {
        "product" => crate::contracts::StorefrontMediaScope::Product,
        "option_value" => crate::contracts::StorefrontMediaScope::OptionValue {
            option_id: ProductOptionId::from_uuid(row.option_id.ok_or_else(|| {
                ApplicationError::Unexpected(anyhow::anyhow!(
                    "database contains an Option Value media row without an Option"
                ))
            })?),
            option_value_id: ProductOptionValueId::from_uuid(row.option_value_id.ok_or_else(
                || {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "database contains an Option Value media row without an Option Value"
                    ))
                },
            )?),
        },
        "variant" => crate::contracts::StorefrontMediaScope::Variant {
            product_variant_id: ProductVariantId::from_uuid(row.product_variant_id.ok_or_else(
                || {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "database contains a Variant media row without a Variant"
                    ))
                },
            )?),
        },
        _ => {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "database contains an invalid Product media scope"
            )));
        }
    };
    let kind = match row.kind.as_str() {
        "image" => MediaKind::Image,
        "video" => MediaKind::Video,
        _ => {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "database contains an invalid Media kind"
            )));
        }
    };
    Ok(StorefrontMediaAsset {
        id: MediaAssetId::from_uuid(row.id),
        scope,
        media_type: row.media_type,
        kind,
        alt_text: row.alt_text,
        position: u16::try_from(row.position).map_err(|_| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "database contains an invalid Media position"
            ))
        })?,
        url: row.url,
    })
}

fn apply_metadata_attachments(
    mut metadata: serde_json::Value,
    attachments: Vec<MetadataAttachmentRow>,
) -> Result<serde_json::Value, ApplicationError> {
    for attachment in attachments {
        let public_url = attachment.public_url.ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "ready Product metadata Media attachment has no public URL"
            ))
        })?;
        let node = metadata.pointer_mut(&attachment.meta_path).ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!(
                "Product metadata Media attachment points to a missing metadata path"
            ))
        })?;
        let Some(node) = node.as_object_mut() else {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "Product metadata Media attachment does not point to an object"
            )));
        };
        let expected_asset_id = attachment.asset_id.to_string();
        if node
            .get("media_asset_id")
            .and_then(serde_json::Value::as_str)
            != Some(expected_asset_id.as_str())
        {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "Product metadata Media attachment does not match its metadata reference"
            )));
        }
        node.insert(
            "media_type".into(),
            serde_json::Value::String(attachment.media_type),
        );
        node.insert("url".into(), serde_json::Value::String(public_url));
    }
    Ok(metadata)
}

async fn variant_selected_options(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MachineActor,
    product_id: ProductId,
) -> Result<HashMap<Uuid, Vec<StorefrontSelectedOption>>, ApplicationError> {
    let rows: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT selection.variant_id, selection.option_id, selection.option_value_id \
         FROM chaos_commerce.variant_selected_options AS selection \
         INNER JOIN chaos_commerce.product_options AS option \
           ON option.store_id = selection.store_id \
          AND option.product_id = selection.product_id \
          AND option.id = selection.option_id \
         INNER JOIN chaos_commerce.product_option_values AS value \
           ON value.store_id = selection.store_id \
          AND value.product_id = selection.product_id \
          AND value.option_id = selection.option_id \
          AND value.id = selection.option_value_id \
         WHERE selection.store_id = $1 \
           AND selection.product_id = $2 \
           AND option.archived_at IS NULL \
           AND value.archived_at IS NULL \
         ORDER BY selection.variant_id ASC, option.position ASC",
    )
    .bind(actor.store_id.as_uuid())
    .bind(product_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let mut by_variant: HashMap<Uuid, Vec<StorefrontSelectedOption>> = HashMap::new();
    for (variant_id, option_id, option_value_id) in rows {
        by_variant
            .entry(variant_id)
            .or_default()
            .push(StorefrontSelectedOption {
                option_id: ProductOptionId::from_uuid(option_id),
                option_value_id: ProductOptionValueId::from_uuid(option_value_id),
            });
    }
    Ok(by_variant)
}

async fn variant_selected_options_for_products(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MachineActor,
    product_ids: &[Uuid],
) -> Result<HashMap<(Uuid, Uuid), Vec<StorefrontSelectedOption>>, ApplicationError> {
    if product_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT selection.product_id, selection.variant_id, selection.option_id, selection.option_value_id \
         FROM chaos_commerce.variant_selected_options AS selection \
         INNER JOIN chaos_commerce.product_options AS option \
           ON option.store_id = selection.store_id \
          AND option.product_id = selection.product_id \
          AND option.id = selection.option_id \
         INNER JOIN chaos_commerce.product_option_values AS value \
           ON value.store_id = selection.store_id \
          AND value.product_id = selection.product_id \
          AND value.option_id = selection.option_id \
          AND value.id = selection.option_value_id \
         WHERE selection.store_id = $1 \
           AND selection.product_id = ANY($2::uuid[]) \
           AND option.archived_at IS NULL \
           AND value.archived_at IS NULL \
         ORDER BY selection.product_id ASC, selection.variant_id ASC, option.position ASC",
    )
    .bind(actor.store_id.as_uuid())
    .bind(product_ids)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    let mut by_variant: HashMap<(Uuid, Uuid), Vec<StorefrontSelectedOption>> = HashMap::new();
    for (product_id, variant_id, option_id, option_value_id) in rows {
        by_variant
            .entry((product_id, variant_id))
            .or_default()
            .push(StorefrontSelectedOption {
                option_id: ProductOptionId::from_uuid(option_id),
                option_value_id: ProductOptionValueId::from_uuid(option_value_id),
            });
    }
    Ok(by_variant)
}

#[cfg(test)]
mod tests {
    use crate::contracts::{MachineActor, StorefrontCatalogRepository};
    use chaos_domain::store::{PublishableKeyId, SalesChannelId, StoreId};
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn serves_only_active_published_and_priced_rows_in_the_resolved_store() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let runtime_pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("SET ROLE chaos_runtime")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .unwrap();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let other_channel_id = SalesChannelId::new();
        let visible_product_id = ProductId::new();
        let visible_variant_id = ProductVariantId::new();
        let draft_product_id = ProductId::new();
        let draft_variant_id = ProductVariantId::new();
        let other_product_id = ProductId::new();
        let other_variant_id = ProductVariantId::new();
        let price_list_id = Uuid::now_v7();
        let other_price_list_id = Uuid::now_v7();
        for id in [store_id, other_store_id] {
            sqlx::query(
                "INSERT INTO chaos_commerce.stores \
                 (id, name, status) \
                 VALUES ($1, 'Storefront Test', 'active')",
            )
            .bind(id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (id, store, name) in [
            (channel_id, store_id, "Web"),
            (other_channel_id, other_store_id, "Other Web"),
        ] {
            sqlx::query(
                "INSERT INTO chaos_commerce.channels \
                 (id, store_id, name, origin) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(id.as_uuid())
            .bind(store.as_uuid())
            .bind(name)
            .bind(format!(
                "https://{}.storefront.example.test/",
                id.as_uuid().simple()
            ))
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (product, variant, store, handle, status) in [
            (
                visible_product_id,
                visible_variant_id,
                store_id,
                "visible-shirt",
                "active",
            ),
            (
                draft_product_id,
                draft_variant_id,
                store_id,
                "draft-shirt",
                "draft",
            ),
            (
                other_product_id,
                other_variant_id,
                other_store_id,
                "other-shirt",
                "active",
            ),
        ] {
            sqlx::query(
                "INSERT INTO chaos_commerce.products \
                 (id, store_id, handle, title, description, status) \
                 VALUES ($1, $2, $3, 'Shirt', 'Safe description', \
                         $4::chaos_commerce.product_status)",
            )
            .bind(product.as_uuid())
            .bind(store.as_uuid())
            .bind(handle)
            .bind(status)
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO chaos_commerce.product_variants \
                 (id, store_id, product_id, title, status) \
                 VALUES ($1, $2, $3, 'Default', 'active')",
            )
            .bind(variant.as_uuid())
            .bind(store.as_uuid())
            .bind(product.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (product, store, channel) in [
            (visible_product_id, store_id, channel_id),
            (draft_product_id, store_id, channel_id),
            (other_product_id, other_store_id, other_channel_id),
        ] {
            sqlx::query(
                "INSERT INTO chaos_commerce.product_publications \
                 (store_id, product_id, channel_id) \
                 VALUES ($1, $2, $3)",
            )
            .bind(store.as_uuid())
            .bind(product.as_uuid())
            .bind(channel.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (list, store, code) in [
            (price_list_id, store_id, "retail"),
            (other_price_list_id, other_store_id, "other-retail"),
        ] {
            sqlx::query(
                "INSERT INTO chaos_commerce.price_lists \
                 (id, store_id, code, name, currency, status) \
                 VALUES ($1, $2, $3, 'Retail', 'USD', 'active')",
            )
            .bind(list)
            .bind(store.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (list, store, variant, amount) in [
            (price_list_id, store_id, visible_variant_id, 2500_i64),
            (price_list_id, store_id, draft_variant_id, 9900_i64),
            (
                other_price_list_id,
                other_store_id,
                other_variant_id,
                100_i64,
            ),
        ] {
            sqlx::query(
                "INSERT INTO chaos_commerce.price_list_items \
                 (id, store_id, price_list_id, \
                  product_variant_id, amount_minor) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(Uuid::now_v7())
            .bind(store.as_uuid())
            .bind(list)
            .bind(variant.as_uuid())
            .bind(amount)
            .execute(&owner_pool)
            .await
            .unwrap();
        }

        // Approved rating 4 and 5 average to 4.5; a pending review is
        // excluded despite outweighing them, and a reply carries no rating
        // to average in the first place.
        for (rating, status, parent, approved_at) in [
            (
                5_i16,
                "approved",
                None::<Uuid>,
                Some(time::OffsetDateTime::now_utc()),
            ),
            (
                4_i16,
                "approved",
                None::<Uuid>,
                Some(time::OffsetDateTime::now_utc()),
            ),
            (1_i16, "pending", None::<Uuid>, None),
        ] {
            let review_id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO chaos_commerce.reviews \
                 (id, store_id, product_id, parent_review_id, rating, content, author_name, \
                  status, approved_at) \
                 VALUES ($1, $2, $3, $4, $5, 'Solid shirt', 'Tester', \
                         $6::chaos_commerce.review_status, $7)",
            )
            .bind(review_id)
            .bind(store_id.as_uuid())
            .bind(visible_product_id.as_uuid())
            .bind(parent)
            .bind(rating)
            .bind(status)
            .bind(approved_at)
            .execute(&owner_pool)
            .await
            .unwrap();
        }

        let actor = MachineActor {
            publishable_key_id: PublishableKeyId::new(),
            store_id,
            channel_id: Some(channel_id),
        };
        let indexer = crate::adapters::postgres::PostgresSearchIndexer::new(runtime_pool.clone());
        assert!(
            indexer
                .run_batch(100, time::OffsetDateTime::now_utc())
                .await
                .unwrap()
                >= 3
        );
        let repository = PostgresStorefrontCatalogRepository::new(runtime_pool);
        let products = repository
            .list_products(&actor, None, None, None, None, 20)
            .await
            .unwrap();
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].id, visible_product_id);
        assert_eq!(products[0].variants.len(), 1);
        assert_eq!(products[0].variants[0].amount_minor, 2500);
        assert_eq!(
            products[0].rating,
            Some(StorefrontRatingSummary {
                average: 4.5,
                count: 2
            })
        );
        let searched = repository
            .list_products(&actor, None, Some("visible"), None, None, 20)
            .await
            .unwrap();
        assert_eq!(searched.len(), 1);
        assert_eq!(searched[0].id, visible_product_id);
        assert!(
            repository
                .list_products(&actor, None, Some("missing"), None, None, 20)
                .await
                .unwrap()
                .is_empty()
        );
        let rebuilt: i64 = sqlx::query_scalar("SELECT chaos_commerce.rebuild_store_products($1)")
            .bind(store_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
        assert_eq!(rebuilt, 2);
        let indexed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM chaos_commerce.product_documents \
             WHERE store_id = $1",
        )
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(indexed, 2);
        let visible = repository
            .get_product_by_handle(&actor, None, "visible-shirt")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            visible.rating,
            Some(StorefrontRatingSummary {
                average: 4.5,
                count: 2
            })
        );
        assert!(
            repository
                .get_product_by_handle(&actor, None, "draft-shirt")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            repository
                .get_product_by_handle(&actor, None, "other-shirt")
                .await
                .unwrap()
                .is_none()
        );
    }
}
