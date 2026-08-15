use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        MachineActor, StorefrontCatalogProduct, StorefrontCatalogRepository,
        StorefrontCatalogVariant,
    },
};
use chaos_domain::{
    CurrencyCode,
    catalog::{ProductId, ProductVariantId},
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
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(actor.merchant_account_id.as_uuid().to_string())
            .execute(&mut *transaction)
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
        let rows = sqlx::query_as::<_, (Uuid, String, Option<String>, bool, i64, String, bool)>(
            "WITH selected_price_list AS ( \
                 SELECT price_list.id, price_list.currency::text, price_list.tax_inclusive \
                 FROM pricing.price_lists AS price_list \
                 INNER JOIN merchant.stores AS store \
                   ON store.merchant_account_id = price_list.merchant_account_id \
                  AND store.id = price_list.store_id \
                 INNER JOIN merchant.sales_channels AS channel \
                   ON channel.merchant_account_id = store.merchant_account_id \
                  AND channel.store_id = store.id \
                  AND channel.id = $3 \
                 INNER JOIN merchant.store_currencies AS store_currency \
                   ON store_currency.merchant_account_id = store.merchant_account_id \
                  AND store_currency.store_id = store.id \
                  AND store_currency.currency = price_list.currency \
                 WHERE price_list.merchant_account_id = $1 \
                   AND price_list.store_id = $2 \
                   AND price_list.status = 'active' \
                   AND store.status = 'active' \
                   AND channel.status = 'active' \
                   AND store_currency.enabled \
                   AND price_list.currency = COALESCE($5::char(3), store.default_currency) \
                   AND (price_list.starts_at IS NULL OR price_list.starts_at <= CURRENT_TIMESTAMP) \
                   AND (price_list.ends_at IS NULL OR price_list.ends_at > CURRENT_TIMESTAMP) \
                 ORDER BY price_list.starts_at DESC NULLS LAST, price_list.id ASC \
                 LIMIT 1 \
             ) \
             SELECT variant.id, variant.title, variant.sku::text, variant.requires_shipping, \
                    price.amount_minor, selected.currency, selected.tax_inclusive \
             FROM catalog.product_variants AS variant \
             INNER JOIN selected_price_list AS selected ON true \
             INNER JOIN pricing.prices AS price \
               ON price.merchant_account_id = variant.merchant_account_id \
              AND price.store_id = variant.store_id \
              AND price.price_list_id = selected.id \
              AND price.product_variant_id = variant.id \
             WHERE variant.merchant_account_id = $1 \
               AND variant.store_id = $2 \
               AND variant.product_id = $4 \
               AND variant.status = 'active' \
             ORDER BY variant.id ASC",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(actor.sales_channel_id.map(|id| id.as_uuid()))
        .bind(product_id.as_uuid())
        .bind(currency.map(|value| value.as_str().to_owned()))
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;

        rows.into_iter()
            .map(
                |(id, title, sku, requires_shipping, amount_minor, currency, tax_inclusive)| {
                    Ok(StorefrontCatalogVariant {
                        id: ProductVariantId::from_uuid(id),
                        title,
                        sku,
                        requires_shipping,
                        amount_minor,
                        currency: CurrencyCode::parse(&currency).map_err(|_| {
                            ApplicationError::Unexpected(anyhow::anyhow!(
                                "database contains an invalid currency"
                            ))
                        })?,
                        tax_inclusive,
                    })
                },
            )
            .collect()
    }
}

#[async_trait]
impl StorefrontCatalogRepository for PostgresStorefrontCatalogRepository {
    async fn list_products(
        &self,
        actor: &MachineActor,
        currency: Option<CurrencyCode>,
        query: Option<&str>,
        after: Option<ProductId>,
        limit: u16,
    ) -> Result<Vec<StorefrontCatalogProduct>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let mut scan_after = after;
        let mut products = Vec::with_capacity(usize::from(limit));
        while products.len() < usize::from(limit) {
            let rows = sqlx::query_as::<_, (Uuid, String, String, String)>(
                "SELECT product.id, product.handle::text, product.title, product.description \
                 FROM catalog.products AS product \
                 INNER JOIN merchant.stores AS store \
                   ON store.merchant_account_id = product.merchant_account_id \
                  AND store.id = product.store_id \
                 INNER JOIN merchant.sales_channels AS channel \
                   ON channel.merchant_account_id = product.merchant_account_id \
                  AND channel.store_id = product.store_id \
                  AND channel.id = $3 \
                 INNER JOIN catalog.product_publications AS publication \
                   ON publication.merchant_account_id = product.merchant_account_id \
                  AND publication.store_id = product.store_id \
                  AND publication.product_id = product.id \
                  AND publication.sales_channel_id = channel.id \
                 INNER JOIN search.product_documents AS search_document \
                   ON search_document.merchant_account_id = product.merchant_account_id \
                  AND search_document.store_id = product.store_id \
                  AND search_document.product_id = product.id \
                 WHERE product.merchant_account_id = $1 \
                   AND product.store_id = $2 \
                   AND store.status = 'active' \
                   AND channel.status = 'active' \
                   AND product.status = 'active' \
                   AND ($5::text IS NULL OR search_document.document @@ websearch_to_tsquery('simple', $5)) \
                   AND ($4::uuid IS NULL OR product.id > $4) \
                 ORDER BY product.id ASC \
                 LIMIT 100",
            )
            .bind(actor.merchant_account_id.as_uuid())
            .bind(actor.store_id.as_uuid())
            .bind(actor.sales_channel_id.map(|id| id.as_uuid()))
            .bind(scan_after.map(ProductId::as_uuid))
            .bind(query)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
            if rows.is_empty() {
                break;
            }
            let rows_len = rows.len();
            for (id, handle, title, description) in rows {
                let id = ProductId::from_uuid(id);
                scan_after = Some(id);
                let variants = Self::variants(&mut transaction, actor, id, currency).await?;
                if !variants.is_empty() {
                    products.push(StorefrontCatalogProduct {
                        id,
                        handle,
                        title,
                        description,
                        variants,
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
        let row = sqlx::query_as::<_, (Uuid, String, String, String)>(
            "SELECT product.id, product.handle::text, product.title, product.description \
             FROM catalog.products AS product \
             INNER JOIN merchant.stores AS store \
               ON store.merchant_account_id = product.merchant_account_id \
              AND store.id = product.store_id \
             INNER JOIN merchant.sales_channels AS channel \
               ON channel.merchant_account_id = product.merchant_account_id \
              AND channel.store_id = product.store_id \
              AND channel.id = $3 \
             INNER JOIN catalog.product_publications AS publication \
               ON publication.merchant_account_id = product.merchant_account_id \
              AND publication.store_id = product.store_id \
              AND publication.product_id = product.id \
              AND publication.sales_channel_id = channel.id \
             WHERE product.merchant_account_id = $1 \
               AND product.store_id = $2 \
               AND product.handle = $4 \
               AND store.status = 'active' \
               AND channel.status = 'active' \
               AND product.status = 'active'",
        )
        .bind(actor.merchant_account_id.as_uuid())
        .bind(actor.store_id.as_uuid())
        .bind(actor.sales_channel_id.map(|id| id.as_uuid()))
        .bind(handle)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let Some((id, handle, title, description)) = row else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let id = ProductId::from_uuid(id);
        let variants = Self::variants(&mut transaction, actor, id, currency).await?;
        transaction.commit().await.map_err(database_error)?;
        if variants.is_empty() {
            return Ok(None);
        }
        Ok(Some(StorefrontCatalogProduct {
            id,
            handle,
            title,
            description,
            variants,
        }))
    }
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use chaos_application::ports::{MachineActor, StorefrontCatalogRepository};
    use chaos_domain::merchant::{
        ApiKeyClass, ApiKeyId, ApiKeyMode, ApiKeyScope, MerchantAccountId, SalesChannelId, StoreId,
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
        let account_id = MerchantAccountId::new();
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
        let suffix = Uuid::now_v7().simple().to_string();

        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Storefront Test')",
        )
        .bind(account_id.as_uuid())
        .bind(format!("storefront-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, code) in [
            (store_id, "storefront"),
            (other_store_id, "other-storefront"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.stores \
                 (id, merchant_account_id, code, name, status) \
                 VALUES ($1, $2, $3, 'Storefront Test', 'active')",
            )
            .bind(id.as_uuid())
            .bind(account_id.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO merchant.store_currencies \
                 (merchant_account_id, store_id, currency) VALUES ($1, $2, 'USD')",
            )
            .bind(account_id.as_uuid())
            .bind(id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (id, store, code) in [
            (channel_id, store_id, "web"),
            (other_channel_id, other_store_id, "other-web"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.sales_channels \
                 (id, merchant_account_id, store_id, code, name, kind, is_default) \
                 VALUES ($1, $2, $3, $4, 'Web', 'web', true)",
            )
            .bind(id.as_uuid())
            .bind(account_id.as_uuid())
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
                "INSERT INTO catalog.products \
                 (id, merchant_account_id, store_id, handle, title, description, status) \
                 VALUES ($1, $2, $3, $4, 'Shirt', 'Safe description', \
                         $5::catalog.product_status)",
            )
            .bind(product.as_uuid())
            .bind(account_id.as_uuid())
            .bind(store.as_uuid())
            .bind(handle)
            .bind(status)
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO catalog.product_variants \
                 (id, merchant_account_id, store_id, product_id, title, status) \
                 VALUES ($1, $2, $3, $4, 'Default', 'active')",
            )
            .bind(variant.as_uuid())
            .bind(account_id.as_uuid())
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
                "INSERT INTO catalog.product_publications \
                 (merchant_account_id, store_id, product_id, sales_channel_id) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(account_id.as_uuid())
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
                "INSERT INTO pricing.price_lists \
                 (id, merchant_account_id, store_id, code, name, currency, status) \
                 VALUES ($1, $2, $3, $4, 'Retail', 'USD', 'active')",
            )
            .bind(list)
            .bind(account_id.as_uuid())
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
                "INSERT INTO pricing.prices \
                 (id, merchant_account_id, store_id, price_list_id, \
                  product_variant_id, amount_minor) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(Uuid::now_v7())
            .bind(account_id.as_uuid())
            .bind(store.as_uuid())
            .bind(list)
            .bind(variant.as_uuid())
            .bind(amount)
            .execute(&owner_pool)
            .await
            .unwrap();
        }

        let actor = MachineActor {
            api_key_id: ApiKeyId::new(),
            merchant_account_id: account_id,
            store_id,
            sales_channel_id: Some(channel_id),
            class: ApiKeyClass::Publishable,
            mode: ApiKeyMode::Live,
            scopes: vec![ApiKeyScope::CatalogRead],
        };
        let indexer = crate::repositories::PostgresSearchIndexer::new(runtime_pool.clone());
        assert!(
            indexer
                .run_batch(Uuid::now_v7(), 100, time::OffsetDateTime::now_utc())
                .await
                .unwrap()
                >= 6
        );
        let repository = PostgresStorefrontCatalogRepository::new(runtime_pool);
        let products = repository
            .list_products(&actor, None, None, None, 20)
            .await
            .unwrap();
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].id, visible_product_id);
        assert_eq!(products[0].variants.len(), 1);
        assert_eq!(products[0].variants[0].amount_minor, 2500);
        let searched = repository
            .list_products(&actor, None, Some("visible"), None, 20)
            .await
            .unwrap();
        assert_eq!(searched.len(), 1);
        assert_eq!(searched[0].id, visible_product_id);
        assert!(
            repository
                .list_products(&actor, None, Some("missing"), None, 20)
                .await
                .unwrap()
                .is_empty()
        );
        let rebuilt: i64 = sqlx::query_scalar("SELECT search.rebuild_store_products($1, $2)")
            .bind(account_id.as_uuid())
            .bind(store_id.as_uuid())
            .fetch_one(&owner_pool)
            .await
            .unwrap();
        assert_eq!(rebuilt, 2);
        let indexed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM search.product_documents \
             WHERE merchant_account_id = $1 AND store_id = $2",
        )
        .bind(account_id.as_uuid())
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
