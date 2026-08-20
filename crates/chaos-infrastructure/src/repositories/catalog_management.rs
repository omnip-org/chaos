use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, CatalogManagementTransaction, CatalogManagementUnitOfWork, IdempotencyRequest,
        ProductLifecycleSnapshot,
    },
};
use chaos_domain::{
    catalog::{CatalogMetadata, ProductContent, ProductId, ProductStatus},
    merchant::{SalesChannelId, StoreId},
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

#[derive(Clone)]
pub struct PostgresCatalogManagementUnitOfWork {
    pool: PgPool,
}

impl PostgresCatalogManagementUnitOfWork {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

struct PostgresCatalogManagementTransaction {
    transaction: Transaction<'static, Postgres>,
    store_id: StoreId,
    product_id: ProductId,
}

#[async_trait]
impl CatalogManagementUnitOfWork for PostgresCatalogManagementUnitOfWork {
    async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        product_id: ProductId,
    ) -> Result<Box<dyn CatalogManagementTransaction>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(unexpected_database_error)?;
        sqlx::query("SELECT set_config('app.user_id', $1, true)")
            .bind(actor.audit_user_id().as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(unexpected_database_error)?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(unexpected_database_error)?;
        Ok(Box::new(PostgresCatalogManagementTransaction {
            transaction,
            store_id,
            product_id,
        }))
    }
}

#[async_trait]
impl CatalogManagementTransaction for PostgresCatalogManagementTransaction {
    async fn reserve_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
    ) -> Result<Option<ProductId>, ApplicationError> {
        let Some(body) = idempotency::reserve(
            &mut self.transaction,
            &IdempotencyScope::Store(self.store_id.as_uuid()),
            operation,
            request,
        )
        .await?
        else {
            return Ok(None);
        };
        body.get("data")
            .and_then(|data| data.get("id"))
            .and_then(|id| id.as_str())
            .and_then(|id| Uuid::parse_str(id).ok())
            .map(ProductId::from_uuid)
            .map(Some)
            .ok_or_else(|| {
                ApplicationError::Unexpected(anyhow::anyhow!(
                    "completed idempotency record has no product mutation response"
                ))
            })
    }

    async fn load_lifecycle(
        &mut self,
    ) -> Result<Option<ProductLifecycleSnapshot>, ApplicationError> {
        let row = sqlx::query_as::<_, (String, i64)>(
            "SELECT product.status::text, (\
                 SELECT count(*) FROM catalog.product_variants AS variant \
                 WHERE variant.store_id = product.store_id \
                   AND variant.product_id = product.id\
             ) \
             FROM catalog.products AS product \
             WHERE product.store_id = $1 AND product.id = $2 \
             FOR UPDATE",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.product_id.as_uuid())
        .fetch_optional(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        row.map(|(status, variant_count)| {
            Ok(ProductLifecycleSnapshot {
                status: ProductStatus::parse(&status).ok_or_else(|| {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "database contains unknown product status"
                    ))
                })?,
                variant_count: u32::try_from(variant_count).map_err(|_| {
                    ApplicationError::Unexpected(anyhow::anyhow!(
                        "product variant count is out of range"
                    ))
                })?,
            })
        })
        .transpose()
    }

    async fn update_content(&mut self, content: &ProductContent) -> Result<bool, ApplicationError> {
        let result = sqlx::query(
            "UPDATE catalog.products \
             SET handle = $3, title = $4, description = $5, metadata = $6::jsonb, \
                 updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.product_id.as_uuid())
        .bind(content.handle().as_str())
        .bind(content.title())
        .bind(content.description())
        .bind(content.metadata().map(CatalogMetadata::as_str))
        .execute(&mut *self.transaction)
        .await
        .map_err(map_catalog_write_error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn set_status(&mut self, status: ProductStatus) -> Result<(), ApplicationError> {
        let result = sqlx::query(
            "UPDATE catalog.products \
             SET status = $3::catalog.product_status, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.product_id.as_uuid())
        .bind(status.as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "locked product disappeared during lifecycle mutation"
            )))
        }
    }

    async fn active_channel_exists(
        &mut self,
        sales_channel_id: SalesChannelId,
    ) -> Result<bool, ApplicationError> {
        sqlx::query_scalar(
            "SELECT EXISTS (\
                SELECT 1 FROM merchant.sales_channels \
                WHERE store_id = $1 AND id = $2 \
                  AND status = 'active'\
             )",
        )
        .bind(self.store_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .fetch_one(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)
    }

    async fn publish(&mut self, sales_channel_id: SalesChannelId) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO catalog.product_publications \
             (store_id, product_id, sales_channel_id) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.product_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .execute(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        Ok(())
    }

    async fn unpublish(
        &mut self,
        sales_channel_id: SalesChannelId,
    ) -> Result<(), ApplicationError> {
        sqlx::query(
            "DELETE FROM catalog.product_publications \
             WHERE store_id = $1 \
               AND product_id = $2 AND sales_channel_id = $3",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.product_id.as_uuid())
        .bind(sales_channel_id.as_uuid())
        .execute(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        Ok(())
    }

    async fn complete_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
        product_id: ProductId,
    ) -> Result<(), ApplicationError> {
        idempotency::complete(
            &mut self.transaction,
            &IdempotencyScope::Store(self.store_id.as_uuid()),
            operation,
            request,
            200,
            json!({ "data": { "id": product_id.as_uuid() } }),
        )
        .await
    }

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError> {
        self.transaction
            .commit()
            .await
            .map_err(unexpected_database_error)
    }
}

fn map_catalog_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error
        && database_error.constraint() == Some("products_store_id_handle_key")
    {
        return ApplicationError::Conflict {
            code: "product_handle_taken",
            message: "the product handle is already in use for this Store",
        };
    }
    unexpected_database_error(error)
}

fn unexpected_database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chaos_application::{
        catalog::{
            CatalogManagement, ChangeProductStatusInput, ProductPublicationInput,
            UpdateProductInput,
        },
        merchant::MerchantQueries,
        ports::{AdminActor, IdempotencyRequest},
    };
    use chaos_domain::{catalog::ProductVariantId, identity::UserId, merchant::SalesChannelId};
    use sqlx::postgres::PgPoolOptions;

    use crate::repositories::PostgresMerchantReadRepository;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn manages_product_content_lifecycle_and_publication_atomically() {
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
        let owner_id = UserId::new();
        let store_id = StoreId::new();
        let other_store_id = StoreId::new();
        let product_id = ProductId::new();
        let empty_product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let channel_id = SalesChannelId::new();
        let other_channel_id = SalesChannelId::new();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_id.as_uuid())
            .bind(format!("catalog-manage-owner-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        for (id, code) in [(store_id, "manage"), (other_store_id, "manage-other")] {
            sqlx::query(
                "INSERT INTO merchant.stores (id, code, name) \
                 VALUES ($1, $2, 'Managed Store')",
            )
            .bind(id.as_uuid())
            .bind(format!("{code}-{suffix}"))
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO merchant.store_memberships (store_id, user_id, role) \
             VALUES ($1, $2, 'owner')",
        )
        .bind(store_id.as_uuid())
        .bind(owner_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        for (id, owning_store, code) in [
            (channel_id, store_id, "web"),
            (other_channel_id, other_store_id, "other-web"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.sales_channels \
                 (id, store_id, code, name, kind) \
                 VALUES ($1, $2, $3, 'Web', 'web')",
            )
            .bind(id.as_uuid())
            .bind(owning_store.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        for (id, handle) in [
            (product_id, "managed-product"),
            (empty_product_id, "empty-product"),
        ] {
            sqlx::query(
                "INSERT INTO catalog.products \
                 (id, store_id, handle, title) \
                 VALUES ($1, $2, $3, 'Managed Product')",
            )
            .bind(id.as_uuid())
            .bind(store_id.as_uuid())
            .bind(handle)
            .execute(&owner_pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, store_id, product_id, title, sku) \
             VALUES ($1, $2, $3, 'Default', 'MANAGED-SKU')",
        )
        .bind(variant_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let directory = MerchantQueries::new(Arc::new(PostgresMerchantReadRepository::new(
            runtime_pool.clone(),
        )));
        let owner = directory.authorize(owner_id, store_id).await.unwrap();
        let service = CatalogManagement::new(Arc::new(PostgresCatalogManagementUnitOfWork::new(
            runtime_pool,
        )));
        let request = |key: String, fingerprint| IdempotencyRequest {
            key,
            request_fingerprint: fingerprint,
        };

        assert!(matches!(
            service
                .activate(ChangeProductStatusInput {
                    actor: AdminActor::Store(owner),
                    store_id,
                    product_id: empty_product_id,
                    idempotency: request(format!("empty-{suffix}"), [61; 32]),
                })
                .await,
            Err(ApplicationError::Validation { .. })
        ));
        assert!(matches!(
            service
                .publish(ProductPublicationInput {
                    actor: AdminActor::Store(owner),
                    store_id,
                    product_id,
                    sales_channel_id: channel_id,
                    idempotency: request(format!("draft-publish-{suffix}"), [62; 32]),
                })
                .await,
            Err(ApplicationError::Validation { .. })
        ));

        let update_key = format!("update-{suffix}");
        let updated = service
            .update(UpdateProductInput {
                actor: AdminActor::Store(owner),
                store_id,
                product_id,
                handle: "updated-product".into(),
                title: "Updated Product".into(),
                description: "Updated description".into(),
                metadata: None,
                idempotency: request(update_key.clone(), [63; 32]),
            })
            .await
            .unwrap();
        let replay = service
            .update(UpdateProductInput {
                actor: AdminActor::Store(owner),
                store_id,
                product_id,
                handle: "updated-product".into(),
                title: "Updated Product".into(),
                description: "Updated description".into(),
                metadata: None,
                idempotency: request(update_key, [63; 32]),
            })
            .await
            .unwrap();
        assert_eq!(updated, replay);

        service
            .activate(ChangeProductStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
                product_id,
                idempotency: request(format!("activate-{suffix}"), [64; 32]),
            })
            .await
            .unwrap();
        assert!(matches!(
            service
                .publish(ProductPublicationInput {
                    actor: AdminActor::Store(owner),
                    store_id,
                    product_id,
                    sales_channel_id: other_channel_id,
                    idempotency: request(format!("wrong-channel-{suffix}"), [65; 32]),
                })
                .await,
            Err(ApplicationError::NotFound {
                resource: "sales_channel",
                ..
            })
        ));
        service
            .publish(ProductPublicationInput {
                actor: AdminActor::Store(owner),
                store_id,
                product_id,
                sales_channel_id: channel_id,
                idempotency: request(format!("publish-{suffix}"), [66; 32]),
            })
            .await
            .unwrap();
        service
            .archive(ChangeProductStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
                product_id,
                idempotency: request(format!("archive-{suffix}"), [67; 32]),
            })
            .await
            .unwrap();

        let stored: (String, String, String, i64) = sqlx::query_as(
            "SELECT product.handle::text, product.title, product.status::text, \
                    count(publication.product_id) \
             FROM catalog.products AS product \
             LEFT JOIN catalog.product_publications AS publication \
              ON publication.store_id = product.store_id \
              AND publication.product_id = product.id \
             WHERE product.id = $1 GROUP BY product.id",
        )
        .bind(product_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            stored,
            (
                "updated-product".into(),
                "Updated Product".into(),
                "archived".into(),
                1
            )
        );

        service
            .unpublish(ProductPublicationInput {
                actor: AdminActor::Store(owner),
                store_id,
                product_id,
                sales_channel_id: channel_id,
                idempotency: request(format!("unpublish-{suffix}"), [68; 32]),
            })
            .await
            .unwrap();
        let publications: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog.product_publications WHERE product_id = $1",
        )
        .bind(product_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(publications, 0);

        sqlx::query("DELETE FROM merchant.stores WHERE id = ANY($1)")
            .bind(vec![store_id.as_uuid(), other_store_id.as_uuid()])
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM integration.idempotency_records \
             WHERE scope = 'store' AND scope_id = $1",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(owner_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn publishable_machine_actor_cannot_administer_catalog_under_rls() {
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
        let owner_id = UserId::new();
        let store_id = StoreId::new();
        let product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_id.as_uuid())
            .bind(format!("catalog-manage-machine-owner-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO merchant.stores (id, code, name) \
             VALUES ($1, $2, 'Managed Store')",
        )
        .bind(store_id.as_uuid())
        .bind(format!("manage-machine-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.products \
             (id, store_id, handle, title) \
             VALUES ($1, $2, 'machine-managed-product', 'Machine Managed Product')",
        )
        .bind(product_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, store_id, product_id, title, sku) \
             VALUES ($1, $2, $3, 'Default', 'MACHINE-MANAGED-SKU')",
        )
        .bind(variant_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let service = CatalogManagement::new(Arc::new(PostgresCatalogManagementUnitOfWork::new(
            runtime_pool,
        )));
        let request = |key: String, fingerprint| IdempotencyRequest {
            key,
            request_fingerprint: fingerprint,
        };
        let publishable_machine = AdminActor::Machine(chaos_application::ports::MachineActor {
            api_key_id: chaos_domain::merchant::ApiKeyId::new(),
            store_id,
            sales_channel_id: None,
            class: chaos_domain::merchant::ApiKeyClass::Publishable,
            scopes: vec![chaos_domain::merchant::ApiKeyScope::CatalogRead],
            created_by_user_id: owner_id,
        });

        assert!(matches!(
            service
                .activate(ChangeProductStatusInput {
                    actor: publishable_machine,
                    store_id,
                    product_id,
                    idempotency: request(format!("machine-forbidden-{suffix}"), [70; 32]),
                })
                .await,
            Err(ApplicationError::Forbidden)
        ));

        sqlx::query("DELETE FROM merchant.stores WHERE id = $1")
            .bind(store_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM integration.idempotency_records \
             WHERE scope = 'store' AND scope_id = $1",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(owner_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
