use std::collections::HashMap;

use crate::{
    ApplicationError,
    contracts::{
        MachineActor, StorefrontCatalogProduct, StorefrontCatalogRepository,
        StorefrontCatalogVariant, StorefrontMediaAsset, StorefrontProductCollection,
        StorefrontProductOption, StorefrontProductOptionValue, StorefrontSelectedOption,
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
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Option<String>,
                bool,
                bool,
                i64,
                i64,
                String,
                Option<serde_json::Value>,
            ),
        >(
            "WITH selected_price_list AS ( \
                 SELECT price_list.id, price_list.currency::text \
                 FROM commerce.price_lists AS price_list \
                 INNER JOIN commerce.stores AS store \
                   ON store.id = price_list.store_id \
                 INNER JOIN commerce.store_sales_channels AS channel \
                   ON channel.store_id = store.id \
                  AND channel.id = $2 \
                 WHERE price_list.store_id = $1 \
                   AND price_list.status = 'active' \
                   AND store.status = 'active' \
                   AND channel.status = 'active' \
                   AND price_list.currency = COALESCE($4::char(3), store.currency) \
                   AND (price_list.starts_at IS NULL OR price_list.starts_at <= CURRENT_TIMESTAMP) \
                   AND (price_list.ends_at IS NULL OR price_list.ends_at > CURRENT_TIMESTAMP) \
                 ORDER BY price_list.starts_at DESC NULLS LAST, price_list.id ASC \
                 LIMIT 1 \
             ) \
            SELECT variant.id, variant.title, variant.sku::text, variant.requires_shipping, \
                    variant.track_inventory, variant.on_hand_quantity, \
                    price.amount_minor, selected.currency, \
                    variant.meta \
             FROM commerce.product_variants AS variant \
             INNER JOIN selected_price_list AS selected ON true \
             INNER JOIN commerce.prices AS price \
               ON price.store_id = variant.store_id \
              AND price.price_list_id = selected.id \
              AND price.product_variant_id = variant.id \
             WHERE variant.store_id = $1 \
               AND variant.product_id = $3 \
               AND variant.status = 'active' \
             ORDER BY variant.id ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.sales_channel_id.map(|id| id.as_uuid()))
        .bind(product_id.as_uuid())
        .bind(currency.map(|value| value.as_str().to_owned()))
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;

        let mut selections = variant_selected_options(transaction, actor, product_id).await?;
        rows.into_iter()
            .map(
                |(
                    id,
                    title,
                    sku,
                    requires_shipping,
                    track_inventory,
                    on_hand_quantity,
                    amount_minor,
                    currency,
                    metadata,
                )| {
                    Ok(StorefrontCatalogVariant {
                        id: ProductVariantId::from_uuid(id),
                        title,
                        sku,
                        requires_shipping,
                        track_inventory,
                        on_hand_quantity,
                        amount_minor,
                        currency: CurrencyCode::parse(&currency).map_err(|_| {
                            ApplicationError::Unexpected(anyhow::anyhow!(
                                "database contains an invalid currency"
                            ))
                        })?,
                        selected_options: selections.remove(&id).unwrap_or_default(),
                        metadata,
                    })
                },
            )
            .collect()
    }

    async fn options(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_id: ProductId,
    ) -> Result<Vec<StorefrontProductOption>, ApplicationError> {
        let option_rows = sqlx::query_as::<_, (Uuid, String, i16)>(
            "SELECT id, name::text, position \
             FROM commerce.product_options \
             WHERE store_id = $1 AND product_id = $2 \
             ORDER BY position ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        let value_rows = sqlx::query_as::<_, (Uuid, Uuid, String, i16)>(
            "SELECT id, option_id, value::text, position \
             FROM commerce.product_option_values \
             WHERE store_id = $1 AND product_id = $2 \
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
        let rows = sqlx::query_as::<_, (Uuid, Option<Uuid>, String, String, String, i16, String)>(
            "SELECT id,product_variant_id,media_type,media_kind::text,alt_text,position,public_url FROM commerce.media_assets WHERE store_id=$1 AND product_id=$2 AND status='ready' ORDER BY position,id",
        )
        .bind(actor.store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(StorefrontMediaAsset {
                    id: MediaAssetId::from_uuid(row.0),
                    product_variant_id: row.1.map(ProductVariantId::from_uuid),
                    media_type: row.2,
                    kind: match row.3.as_str() {
                        "image" => MediaKind::Image,
                        "video" => MediaKind::Video,
                        _ => {
                            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                                "database contains an invalid Media kind"
                            )));
                        }
                    },
                    alt_text: row.4,
                    position: u16::try_from(row.5).map_err(|_| {
                        ApplicationError::Unexpected(anyhow::anyhow!(
                            "database contains an invalid Media position"
                        ))
                    })?,
                    url: row.6,
                })
            })
            .collect()
    }

    async fn collections(
        transaction: &mut Transaction<'_, Postgres>,
        actor: &MachineActor,
        product_id: ProductId,
    ) -> Result<Vec<StorefrontProductCollection>, ApplicationError> {
        let rows = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT collection.id, collection.handle::text, collection.title \
             FROM commerce.collection_products AS member \
             INNER JOIN commerce.collections AS collection \
               ON collection.store_id = member.store_id \
              AND collection.id = member.collection_id \
             INNER JOIN commerce.collection_publications AS publication \
               ON publication.store_id = collection.store_id \
              AND publication.collection_id = collection.id \
              AND publication.sales_channel_id = $2 \
             WHERE member.store_id = $1 \
               AND member.product_id = $3 \
               AND collection.status = 'active' \
             ORDER BY collection.handle ASC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.sales_channel_id.map(|id| id.as_uuid()))
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
            let rows = sqlx::query_as::<
                _,
                (Uuid, String, String, String, Option<serde_json::Value>),
            >(
                "SELECT product.id, product.handle::text, product.title, product.description, \
                        product.meta \
                 FROM commerce.products AS product \
                 INNER JOIN commerce.stores AS store \
                   ON store.id = product.store_id \
                 INNER JOIN commerce.store_sales_channels AS channel \
                   ON channel.store_id = product.store_id \
                  AND channel.id = $2 \
                 INNER JOIN commerce.product_publications AS publication \
                   ON publication.store_id = product.store_id \
                  AND publication.product_id = product.id \
                  AND publication.sales_channel_id = channel.id \
                 INNER JOIN commerce.product_documents AS search_document \
                   ON search_document.store_id = product.store_id \
                  AND search_document.product_id = product.id \
                 WHERE product.store_id = $1 \
                   AND store.status = 'active' \
                   AND channel.status = 'active' \
                   AND product.status = 'active' \
                   AND ($4::text IS NULL OR search_document.document @@ websearch_to_tsquery('simple', $4)) \
                   AND ($5::text IS NULL OR EXISTS ( \
                       SELECT 1 FROM commerce.collections AS collection \
                       INNER JOIN commerce.collection_products AS member \
                         ON member.store_id = collection.store_id \
                        AND member.collection_id = collection.id \
                        AND member.product_id = product.id \
                       INNER JOIN commerce.collection_publications AS collection_publication \
                         ON collection_publication.store_id = collection.store_id \
                        AND collection_publication.collection_id = collection.id \
                        AND collection_publication.sales_channel_id = channel.id \
                       WHERE collection.store_id = product.store_id \
                         AND collection.status = 'active' AND collection.handle = $5)) \
                   AND ( \
                       $3::uuid IS NULL \
                       OR ($5::text IS NULL AND product.id > $3) \
                       OR ($5::text IS NOT NULL AND ( \
                           SELECT member.position \
                           FROM commerce.collections AS collection \
                           INNER JOIN commerce.collection_products AS member \
                             ON member.store_id = collection.store_id \
                            AND member.collection_id = collection.id \
                           WHERE collection.store_id = product.store_id \
                             AND collection.handle = $5 AND member.product_id = product.id \
                       ) > ( \
                           SELECT anchor.position \
                           FROM commerce.collections AS collection \
                           INNER JOIN commerce.collection_products AS anchor \
                             ON anchor.store_id = collection.store_id \
                            AND anchor.collection_id = collection.id \
                           WHERE collection.store_id = product.store_id \
                             AND collection.handle = $5 AND anchor.product_id = $3 \
                       )) \
                   ) \
                 ORDER BY CASE WHEN $5::text IS NOT NULL THEN ( \
                     SELECT member.position \
                     FROM commerce.collections AS collection \
                     INNER JOIN commerce.collection_products AS member \
                       ON member.store_id = collection.store_id \
                      AND member.collection_id = collection.id \
                     WHERE collection.store_id = product.store_id \
                       AND collection.handle = $5 AND member.product_id = product.id \
                 ) END ASC, product.id ASC \
                 LIMIT 100",
            )
            .bind(actor.store_id.as_uuid())
            .bind(actor.sales_channel_id.map(|id| id.as_uuid()))
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
            for (id, handle, title, description, metadata) in rows {
                let id = ProductId::from_uuid(id);
                scan_after = Some(id);
                let variants = Self::variants(&mut transaction, actor, id, currency).await?;
                if !variants.is_empty() {
                    let options = Self::options(&mut transaction, actor, id).await?;
                    let media = Self::media(&mut transaction, actor, id).await?;
                    let collections = Self::collections(&mut transaction, actor, id).await?;
                    products.push(StorefrontCatalogProduct {
                        id,
                        handle,
                        title,
                        description,
                        options,
                        variants,
                        media,
                        collections,
                        metadata,
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
        let row = sqlx::query_as::<_, (Uuid, String, String, String, Option<serde_json::Value>)>(
            "SELECT product.id, product.handle::text, product.title, product.description, \
                    product.meta \
             FROM commerce.products AS product \
             INNER JOIN commerce.stores AS store \
               ON store.id = product.store_id \
             INNER JOIN commerce.store_sales_channels AS channel \
               ON channel.store_id = product.store_id \
              AND channel.id = $2 \
             INNER JOIN commerce.product_publications AS publication \
               ON publication.store_id = product.store_id \
              AND publication.product_id = product.id \
              AND publication.sales_channel_id = channel.id \
             WHERE product.store_id = $1 \
               AND product.handle = $3 \
               AND store.status = 'active' \
               AND channel.status = 'active' \
               AND product.status = 'active'",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.sales_channel_id.map(|id| id.as_uuid()))
        .bind(handle)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let Some((id, handle, title, description, metadata)) = row else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let id = ProductId::from_uuid(id);
        let variants = Self::variants(&mut transaction, actor, id, currency).await?;
        let options = Self::options(&mut transaction, actor, id).await?;
        let media = Self::media(&mut transaction, actor, id).await?;
        let collections = Self::collections(&mut transaction, actor, id).await?;
        transaction.commit().await.map_err(database_error)?;
        if variants.is_empty() {
            return Ok(None);
        }
        Ok(Some(StorefrontCatalogProduct {
            id,
            handle,
            title,
            description,
            options,
            variants,
            media,
            collections,
            metadata,
        }))
    }
}

async fn variant_selected_options(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &MachineActor,
    product_id: ProductId,
) -> Result<HashMap<Uuid, Vec<StorefrontSelectedOption>>, ApplicationError> {
    let rows: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT selection.variant_id, selection.option_id, selection.option_value_id \
         FROM commerce.variant_selected_options AS selection \
         INNER JOIN commerce.product_options AS option \
           ON option.store_id = selection.store_id \
          AND option.product_id = selection.product_id \
          AND option.id = selection.option_id \
         WHERE selection.store_id = $1 \
           AND selection.product_id = $2 \
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

#[cfg(test)]
mod tests {
    use crate::contracts::{MachineActor, StorefrontCatalogRepository};
    use chaos_domain::{
        identity::UserId,
        store::{PublishableKeyId, SalesChannelId, StoreId},
    };
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
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        for (id, code) in [
            (store_id, format!("storefront-{suffix}")),
            (other_store_id, format!("other-storefront-{suffix}")),
        ] {
            sqlx::query(
                "INSERT INTO commerce.stores \
                 (id, code, name, status) \
                 VALUES ($1, $2, 'Storefront Test', 'active')",
            )
            .bind(id.as_uuid())
            .bind(&code)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (id, store, code) in [
            (channel_id, store_id, "web"),
            (other_channel_id, other_store_id, "other-web"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.store_sales_channels \
                 (id, store_id, code, name, is_default) \
                 VALUES ($1, $2, $3, 'Web', true)",
            )
            .bind(id.as_uuid())
            .bind(store.as_uuid())
            .bind(code)
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
                "INSERT INTO commerce.products \
                 (id, store_id, handle, title, description, status) \
                 VALUES ($1, $2, $3, 'Shirt', 'Safe description', \
                         $4::commerce.product_status)",
            )
            .bind(product.as_uuid())
            .bind(store.as_uuid())
            .bind(handle)
            .bind(status)
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO commerce.product_variants \
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
                "INSERT INTO commerce.product_publications \
                 (store_id, product_id, sales_channel_id) \
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
                "INSERT INTO commerce.price_lists \
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
                "INSERT INTO commerce.prices \
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

        let actor = MachineActor {
            publishable_key_id: PublishableKeyId::new(),
            store_id,
            sales_channel_id: Some(channel_id),
            created_by_user_id: UserId::new(),
        };
        let indexer = crate::adapters::postgres::PostgresSearchIndexer::new(runtime_pool.clone());
        assert!(
            indexer
                .run_batch(100, time::OffsetDateTime::now_utc())
                .await
                .unwrap()
                >= 6
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
        let rebuilt: i64 = sqlx::query_scalar("SELECT commerce.rebuild_store_products($1)")
            .bind(store_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
        assert_eq!(rebuilt, 2);
        let indexed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commerce.product_documents \
             WHERE store_id = $1",
        )
        .bind(store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(indexed, 2);
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
