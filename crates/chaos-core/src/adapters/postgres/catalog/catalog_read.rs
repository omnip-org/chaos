use crate::{
    ApplicationError,
    contracts::{
        AdminActor, CatalogProductDetail, CatalogProductListItem, CatalogProductOption,
        CatalogProductOptionValue, CatalogProductPublication, CatalogProductVariant,
        CatalogSelectedOption,
    },
    error::database_error,
};
use chaos_domain::{
    catalog::{
        ProductId, ProductOptionId, ProductOptionValueId, ProductStatus, ProductVariantId,
        VariantStatus,
    },
    store::SalesChannelId,
    store::StoreId,
};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresCatalogReadRepository {
    pool: PgPool,
}

impl PostgresCatalogReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl PostgresCatalogReadRepository {
    pub(crate) async fn list_products(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<ProductId>,
        limit: u16,
        query: Option<&str>,
        status: Option<ProductStatus>,
    ) -> Result<Option<Vec<CatalogProductListItem>>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        if !store_exists(&mut transaction, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                i64,
                OffsetDateTime,
                OffsetDateTime,
                i64,
            ),
        >(
            "SELECT product.id, product.handle::text, product.title, product.status::text, \
                    count(variant.id), product.created_at, product.updated_at, product.revision \
             FROM commerce.products AS product \
             LEFT JOIN commerce.product_variants AS variant \
              ON variant.store_id = product.store_id \
              AND variant.product_id = product.id \
              AND variant.status = 'active' \
             LEFT JOIN commerce.product_documents AS document \
              ON document.store_id = product.store_id \
              AND document.product_id = product.id \
             WHERE product.store_id = $1 \
               AND ($2::uuid IS NULL OR product.id > $2) \
               AND ($3::text IS NULL OR product.status = $3::commerce.product_status) \
               AND ($4::text IS NULL OR document.document @@ websearch_to_tsquery('simple', $4)) \
             GROUP BY product.id \
             ORDER BY product.id ASC \
             LIMIT $5",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(ProductId::as_uuid))
        .bind(status.map(ProductStatus::as_str))
        .bind(query)
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;

        rows.into_iter()
            .map(
                |(id, handle, title, status, variant_count, created_at, updated_at, revision)| {
                    Ok(CatalogProductListItem {
                        id: ProductId::from_uuid(id),
                        handle,
                        title,
                        status: parse_product_status(&status)?,
                        variant_count: u32::try_from(variant_count).map_err(|_| {
                            corrupt_database_value("product variant count is out of range")
                        })?,
                        revision,
                        created_at,
                        updated_at,
                    })
                },
            )
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub(crate) async fn get_product(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Option<CatalogProductDetail>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let product = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                String,
                Option<serde_json::Value>,
                OffsetDateTime,
                OffsetDateTime,
                i64,
            ),
        >(
            "SELECT id, handle::text, title, description, status::text, meta, \
                    created_at, updated_at, revision \
             FROM commerce.products \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let Some((
            id,
            handle,
            title,
            description,
            status,
            metadata,
            created_at,
            updated_at,
            revision,
        )) = product
        else {
            return Ok(None);
        };

        let option_rows = sqlx::query_as::<_, (Uuid, String, i16, Option<OffsetDateTime>)>(
            "SELECT id, name::text, position, archived_at \
             FROM commerce.product_options \
             WHERE store_id = $1 AND product_id = $2 \
             ORDER BY position ASC, id ASC",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let value_rows = sqlx::query_as::<_, (Uuid, Uuid, String, i16, Option<OffsetDateTime>)>(
            "SELECT id, option_id, value::text, position, archived_at \
             FROM commerce.product_option_values \
             WHERE store_id = $1 AND product_id = $2 \
             ORDER BY option_id ASC, position ASC, id ASC",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let variant_rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Option<String>,
                String,
                bool,
                Option<serde_json::Value>,
                OffsetDateTime,
                OffsetDateTime,
            ),
        >(
            "SELECT id, title, sku::text, status::text, track_inventory, \
                    meta, created_at, updated_at \
             FROM commerce.product_variants \
             WHERE store_id = $1 AND product_id = $2 \
             ORDER BY id ASC",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let selection_rows = sqlx::query_as::<_, (Uuid, Uuid, String, Uuid, String)>(
            "SELECT selection.variant_id, selection.option_id, option.name::text, \
                    selection.option_value_id, value.value::text \
             FROM commerce.variant_selected_options AS selection \
             INNER JOIN commerce.product_options AS option \
              ON option.store_id = selection.store_id \
              AND option.product_id = selection.product_id \
              AND option.id = selection.option_id \
             INNER JOIN commerce.product_option_values AS value \
              ON value.store_id = selection.store_id \
              AND value.product_id = selection.product_id \
              AND value.option_id = selection.option_id \
              AND value.id = selection.option_value_id \
             WHERE selection.store_id = $1 \
               AND selection.product_id = $2 \
             ORDER BY selection.variant_id ASC, option.position ASC",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;

        let mut options = option_rows
            .into_iter()
            .map(|(id, name, position, archived_at)| {
                Ok(CatalogProductOption {
                    id: ProductOptionId::from_uuid(id),
                    name,
                    position: position_from_database(position)?,
                    archived_at,
                    values: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        for (id, option_id, value, position, archived_at) in value_rows {
            let option = options
                .iter_mut()
                .find(|option| option.id.as_uuid() == option_id)
                .ok_or_else(|| corrupt_database_value("option value has no parent option"))?;
            option.values.push(CatalogProductOptionValue {
                id: ProductOptionValueId::from_uuid(id),
                value,
                position: position_from_database(position)?,
                archived_at,
            });
        }
        let mut variants = variant_rows
            .into_iter()
            .map(
                |(id, title, sku, status, track_inventory, metadata, created_at, updated_at)| {
                    Ok(CatalogProductVariant {
                        id: ProductVariantId::from_uuid(id),
                        title,
                        sku,
                        status: parse_variant_status(&status)?,
                        track_inventory,
                        selected_options: Vec::new(),
                        metadata,
                        created_at,
                        updated_at,
                    })
                },
            )
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        for (variant_id, option_id, option_name, option_value_id, value) in selection_rows {
            let variant = variants
                .iter_mut()
                .find(|variant| variant.id.as_uuid() == variant_id)
                .ok_or_else(|| corrupt_database_value("selection has no parent variant"))?;
            variant.selected_options.push(CatalogSelectedOption {
                option_id: ProductOptionId::from_uuid(option_id),
                option_name,
                option_value_id: ProductOptionValueId::from_uuid(option_value_id),
                value,
            });
        }

        Ok(Some(CatalogProductDetail {
            id: ProductId::from_uuid(id),
            handle,
            title,
            description,
            status: parse_product_status(&status)?,
            revision,
            options,
            variants,
            metadata,
            created_at,
            updated_at,
        }))
    }

    pub(crate) async fn list_product_publications(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Vec<CatalogProductPublication>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let rows = sqlx::query_scalar::<_, Uuid>(
            "SELECT channel_id \
             FROM commerce.product_publications \
             WHERE store_id = $1 AND product_id = $2 \
             ORDER BY channel_id ASC",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(rows
            .into_iter()
            .map(|channel_id| CatalogProductPublication {
                channel_id: SalesChannelId::from_uuid(channel_id),
            })
            .collect())
    }

    pub(crate) async fn product_revision(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Option<i64>, ApplicationError> {
        let mut transaction = self.begin(actor).await?;
        let revision = sqlx::query_scalar::<_, i64>(
            "SELECT revision FROM commerce.products \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(revision)
    }
}

impl PostgresCatalogReadRepository {
    async fn begin(
        &self,
        actor: AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        crate::adapters::postgres::database::set_admin_context(
            &mut transaction,
            actor.audit_user_id(),
            actor.store_id(),
        )
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }
}

async fn store_exists(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM commerce.stores WHERE id = $1)")
        .bind(store_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)
}

fn parse_product_status(value: &str) -> Result<ProductStatus, ApplicationError> {
    ProductStatus::parse(value).ok_or_else(|| corrupt_database_value("unknown product status"))
}

fn parse_variant_status(value: &str) -> Result<VariantStatus, ApplicationError> {
    VariantStatus::parse(value)
        .ok_or_else(|| corrupt_database_value("unknown product variant status"))
}

fn position_from_database(value: i16) -> Result<u16, ApplicationError> {
    u16::try_from(value).map_err(|_| corrupt_database_value("negative Catalog position"))
}

fn corrupt_database_value(message: &'static str) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database invariant violation: {message}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{catalog::CatalogQueries, store::StoreQueries};
    use chaos_domain::{
        catalog::{ProductOptionId, ProductOptionValueId, ProductVariantId},
        identity::UserId,
    };
    use sqlx::postgres::PgPoolOptions;

    use crate::adapters::postgres::PostgresStoreReadRepository;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn reads_paginated_products_and_complete_aggregate_under_rls() {
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
        let user_id = UserId::new();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let mut product_ids = [ProductId::new(), ProductId::new()];
        product_ids.sort_by_key(|id| id.as_uuid());
        let detail_product_id = product_ids[0];
        let option_id = ProductOptionId::new();
        let value_id = ProductOptionValueId::new();
        let variant_id = ProductVariantId::new();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id.as_uuid())
            .bind(format!("catalog-read-owner-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        for id in [store_id, other_store_id] {
            sqlx::query(
                "INSERT INTO commerce.stores (id, name) \
                 VALUES ($1, 'Catalog Read Store')",
            )
            .bind(id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO commerce.store_memberships (store_id, user_id, role) \
             VALUES ($1, $2, 'owner')",
        )
        .bind(store_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, handle, title) in [
            (detail_product_id, "first-shirt", "First Shirt"),
            (product_ids[1], "second-shirt", "Second Shirt"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.products \
                 (id, store_id, handle, title, description) \
                 VALUES ($1, $2, $3, $4, 'Product description')",
            )
            .bind(id.as_uuid())
            .bind(store_id.as_uuid())
            .bind(handle)
            .bind(title)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO commerce.product_options \
             (id, store_id, product_id, name, position) \
             VALUES ($1, $2, $3, 'Color', 0)",
        )
        .bind(option_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(detail_product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.product_option_values \
             (id, store_id, product_id, option_id, value, position) \
             VALUES ($1, $2, $3, $4, 'Blue', 0)",
        )
        .bind(value_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(detail_product_id.as_uuid())
        .bind(option_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.product_variants \
             (id, store_id, product_id, title, sku) \
             VALUES ($1, $2, $3, 'Blue', 'READ-BLUE')",
        )
        .bind(variant_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(detail_product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.variant_selected_options \
             (store_id, product_id, variant_id, option_id, option_value_id) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(store_id.as_uuid())
        .bind(detail_product_id.as_uuid())
        .bind(variant_id.as_uuid())
        .bind(option_id.as_uuid())
        .bind(value_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let memberships = StoreQueries::new(Arc::new(PostgresStoreReadRepository::new(
            runtime_pool.clone(),
        )));
        let owner = memberships.authorize(user_id, store_id).await.unwrap();
        let queries =
            CatalogQueries::new(Arc::new(PostgresCatalogReadRepository::new(runtime_pool)));

        let first_page = queries
            .list_products(AdminActor::Store(owner), store_id, None, 1, None, None)
            .await
            .unwrap();
        assert_eq!(first_page.items.len(), 1);
        assert!(first_page.has_more);
        assert_eq!(first_page.items[0].id, detail_product_id);
        assert_eq!(first_page.items[0].variant_count, 1);
        let second_page = queries
            .list_products(
                AdminActor::Store(owner),
                store_id,
                Some(detail_product_id),
                1,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(second_page.items.len(), 1);
        assert!(!second_page.has_more);

        let detail = queries
            .get_product(AdminActor::Store(owner), store_id, detail_product_id)
            .await
            .unwrap();
        assert_eq!(detail.options.len(), 1);
        assert_eq!(detail.options[0].values[0].value, "Blue");
        assert_eq!(detail.variants.len(), 1);
        assert_eq!(detail.variants[0].selected_options[0].option_name, "Color");
        assert_eq!(detail.variants[0].selected_options[0].value, "Blue");
        assert!(matches!(
            queries
                .get_product(AdminActor::Store(owner), other_store_id, detail_product_id)
                .await,
            Err(ApplicationError::NotFound {
                resource: "product",
                ..
            })
        ));
        assert!(matches!(
            queries
                .list_products(
                    AdminActor::Store(owner),
                    StoreId::new(),
                    None,
                    10,
                    None,
                    None
                )
                .await,
            Err(ApplicationError::NotFound {
                resource: "store",
                ..
            })
        ));

        sqlx::query("DELETE FROM commerce.stores WHERE id = ANY($1)")
            .bind(vec![store_id.as_uuid(), other_store_id.as_uuid()])
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(user_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
