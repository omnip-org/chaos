use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, CatalogProvisioningTransaction, CatalogProvisioningUnitOfWork,
        IdempotencyRequest,
    },
};
use chaos_domain::{
    catalog::{CatalogMetadata, Product, ProductId},
    store::StoreId,
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::repositories::shared::idempotency::{self, IdempotencyScope};

const CREATE_PRODUCT_OPERATION: &str = "products.create.v1";

#[derive(Clone)]
pub struct PostgresCatalogProvisioningUnitOfWork {
    pool: PgPool,
}

impl PostgresCatalogProvisioningUnitOfWork {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

struct PostgresCatalogProvisioningTransaction {
    transaction: Transaction<'static, Postgres>,
    store_id: StoreId,
}

#[async_trait]
impl CatalogProvisioningUnitOfWork for PostgresCatalogProvisioningUnitOfWork {
    async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Box<dyn CatalogProvisioningTransaction>, ApplicationError> {
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
        Ok(Box::new(PostgresCatalogProvisioningTransaction {
            transaction,
            store_id,
        }))
    }
}

#[async_trait]
impl CatalogProvisioningTransaction for PostgresCatalogProvisioningTransaction {
    async fn require_writable_store(&mut self) -> Result<(), ApplicationError> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM commerce.stores WHERE id = $1 AND status = 'active')",
        )
        .bind(self.store_id.as_uuid())
        .fetch_one(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        if exists {
            Ok(())
        } else {
            Err(ApplicationError::NotFound {
                resource: "store",
                id: self.store_id.as_uuid().to_string(),
            })
        }
    }

    async fn reserve_product_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<ProductId>, ApplicationError> {
        let Some(body) = idempotency::reserve(
            &mut self.transaction,
            &IdempotencyScope::Store(self.store_id.as_uuid()),
            CREATE_PRODUCT_OPERATION,
            request,
        )
        .await?
        else {
            return Ok(None);
        };
        let product_id = body
            .get("data")
            .and_then(|data| data.get("id"))
            .and_then(|id| id.as_str())
            .and_then(|id| Uuid::parse_str(id).ok())
            .map(ProductId::from_uuid)
            .ok_or_else(|| {
                ApplicationError::Unexpected(anyhow::anyhow!(
                    "completed idempotency record has no product response"
                ))
            })?;
        Ok(Some(product_id))
    }

    async fn insert_product(&mut self, product: &Product) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO commerce.products \
             (id, store_id, handle, title, description, status, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6::commerce.product_status, $7::jsonb)",
        )
        .bind(product.id().as_uuid())
        .bind(product.store_id().as_uuid())
        .bind(product.handle().as_str())
        .bind(product.title())
        .bind(product.description())
        .bind(product.status().as_str())
        .bind(product.metadata().map(CatalogMetadata::as_str))
        .execute(&mut *self.transaction)
        .await
        .map_err(map_catalog_write_error)?;

        for option in product.options() {
            sqlx::query(
                "INSERT INTO commerce.product_options \
                 (id, store_id, product_id, name, position) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(option.id().as_uuid())
            .bind(product.store_id().as_uuid())
            .bind(product.id().as_uuid())
            .bind(option.name())
            .bind(i16::try_from(option.position()).expect("option position fits SMALLINT"))
            .execute(&mut *self.transaction)
            .await
            .map_err(map_catalog_write_error)?;
            for value in option.values() {
                sqlx::query(
                    "INSERT INTO commerce.product_option_values \
                     (id, store_id, product_id, option_id, value, position) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(value.id().as_uuid())
                .bind(product.store_id().as_uuid())
                .bind(product.id().as_uuid())
                .bind(option.id().as_uuid())
                .bind(value.value())
                .bind(i16::try_from(value.position()).expect("option value position fits SMALLINT"))
                .execute(&mut *self.transaction)
                .await
                .map_err(map_catalog_write_error)?;
            }
        }

        for variant in product.variants() {
            sqlx::query(
                "INSERT INTO commerce.product_variants \
                 (id, store_id, product_id, title, sku, status, \
                  requires_shipping, track_inventory, metadata) \
                 VALUES ($1, $2, $3, $4, $5, $6::commerce.variant_status, $7, $8, $9::jsonb)",
            )
            .bind(variant.id().as_uuid())
            .bind(product.store_id().as_uuid())
            .bind(product.id().as_uuid())
            .bind(variant.title())
            .bind(variant.sku().map(|sku| sku.as_str()))
            .bind(variant.status().as_str())
            .bind(variant.requires_shipping())
            .bind(variant.track_inventory())
            .bind(variant.metadata().map(CatalogMetadata::as_str))
            .execute(&mut *self.transaction)
            .await
            .map_err(map_catalog_write_error)?;
            for selection in variant.selected_options() {
                sqlx::query(
                    "INSERT INTO commerce.variant_selected_options \
                     (store_id, product_id, variant_id, option_id, \
                      option_value_id) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(product.store_id().as_uuid())
                .bind(product.id().as_uuid())
                .bind(variant.id().as_uuid())
                .bind(selection.option_id().as_uuid())
                .bind(selection.option_value_id().as_uuid())
                .execute(&mut *self.transaction)
                .await
                .map_err(map_catalog_write_error)?;
            }
        }
        Ok(())
    }

    async fn complete_product_creation(
        &mut self,
        request: &IdempotencyRequest,
        product_id: ProductId,
    ) -> Result<(), ApplicationError> {
        idempotency::complete(
            &mut self.transaction,
            &IdempotencyScope::Store(self.store_id.as_uuid()),
            CREATE_PRODUCT_OPERATION,
            request,
            201,
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
    if let sqlx::Error::Database(database_error) = &error {
        return match database_error.constraint() {
            Some("products_store_id_handle_key") => ApplicationError::Conflict {
                code: "product_handle_taken",
                message: "the product handle is already in use for this store",
            },
            Some("product_variants_store_sku_key") => ApplicationError::Conflict {
                code: "variant_sku_taken",
                message: "the variant SKU is already in use for this store",
            },
            _ => unexpected_database_error(error),
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
            CreateProduct, CreateProductInput, CreateProductOptionInput,
            CreateProductSelectedOptionInput, CreateProductVariantInput,
        },
        ports::{AdminActor, IdempotencyRequest},
        store::StoreQueries,
    };
    use chaos_domain::{
        identity::UserId,
        store::{StoreId, StoreRole},
    };
    use sqlx::postgres::PgPoolOptions;

    use crate::repositories::PostgresStoreReadRepository;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn creates_a_product_atomically_with_authorization_and_idempotency() {
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
        let owner_user_id = UserId::new();
        let member_user_id = UserId::new();
        let store_id = StoreId::new();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        for (user_id, label) in [(owner_user_id, "owner"), (member_user_id, "member")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(user_id.as_uuid())
                .bind(format!("catalog-{label}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO commerce.stores \
            (id, code, name, region, currency, status) \
             VALUES ($1, $2, 'Catalog Store', 'US', 'USD', 'active')",
        )
        .bind(store_id.as_uuid())
        .bind(format!("catalog-{suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        for (user_id, role) in [(owner_user_id, "owner"), (member_user_id, "member")] {
            sqlx::query(
                "INSERT INTO commerce.store_memberships \
                 (store_id, user_id, role) \
                 VALUES ($1, $2, $3::commerce.store_role)",
            )
            .bind(store_id.as_uuid())
            .bind(user_id.as_uuid())
            .bind(role)
            .execute(&owner_pool)
            .await
            .unwrap();
        }

        let queries = StoreQueries::new(Arc::new(PostgresStoreReadRepository::new(
            runtime_pool.clone(),
        )));
        let owner = queries.authorize(owner_user_id, store_id).await.unwrap();
        assert_eq!(owner.role(), StoreRole::Owner);
        let service = CreateProduct::new(Arc::new(PostgresCatalogProvisioningUnitOfWork::new(
            runtime_pool,
        )));
        let idempotency_key = format!("product-{suffix}");
        let make_input = |actor: chaos_application::store::StoreActor,
                          fingerprint,
                          key: String,
                          handle: &str,
                          sku: &str| CreateProductInput {
            actor: AdminActor::Store(actor),
            store_id,
            handle: handle.into(),
            title: "Classic Shirt".into(),
            description: "A durable everyday shirt.".into(),
            options: vec![
                CreateProductOptionInput {
                    name: "Color".into(),
                    values: vec!["Blue".into(), "Black".into()],
                },
                CreateProductOptionInput {
                    name: "Size".into(),
                    values: vec!["S".into(), "M".into()],
                },
            ],
            variants: vec![CreateProductVariantInput {
                title: "Blue / M".into(),
                sku: Some(sku.into()),
                requires_shipping: true,
                track_inventory: true,
                selected_options: vec![
                    CreateProductSelectedOptionInput {
                        option: "Color".into(),
                        value: "Blue".into(),
                    },
                    CreateProductSelectedOptionInput {
                        option: "Size".into(),
                        value: "M".into(),
                    },
                ],
                metadata: None,
            }],
            metadata: None,
            idempotency: IdempotencyRequest {
                key,
                request_fingerprint: fingerprint,
            },
        };

        let output = service
            .execute(make_input(
                owner,
                [41; 32],
                idempotency_key.clone(),
                "classic-shirt",
                "SHIRT-BLUE-M",
            ))
            .await
            .unwrap();
        let replay = service
            .execute(make_input(
                owner,
                [41; 32],
                idempotency_key.clone(),
                "classic-shirt",
                "SHIRT-BLUE-M",
            ))
            .await
            .unwrap();
        assert_eq!(output.product_id, replay.product_id);
        assert!(matches!(
            service
                .execute(make_input(
                    owner,
                    [42; 32],
                    idempotency_key.clone(),
                    "classic-shirt",
                    "SHIRT-BLUE-M",
                ))
                .await,
            Err(ApplicationError::Conflict {
                code: "idempotency_key_reused",
                ..
            })
        ));
        assert!(matches!(
            service
                .execute(make_input(
                    owner,
                    [43; 32],
                    format!("handle-conflict-{suffix}"),
                    "classic-shirt",
                    "ANOTHER-SKU",
                ))
                .await,
            Err(ApplicationError::Conflict {
                code: "product_handle_taken",
                ..
            })
        ));
        assert!(matches!(
            service
                .execute(make_input(
                    owner,
                    [44; 32],
                    format!("sku-conflict-{suffix}"),
                    "another-shirt",
                    "shirt-blue-m",
                ))
                .await,
            Err(ApplicationError::Conflict {
                code: "variant_sku_taken",
                ..
            })
        ));

        let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT \
                (SELECT count(*) FROM commerce.products WHERE id = $1), \
                (SELECT count(*) FROM commerce.product_options WHERE product_id = $1), \
                (SELECT count(*) FROM commerce.product_option_values WHERE product_id = $1), \
                (SELECT count(*) FROM commerce.product_variants WHERE product_id = $1), \
                (SELECT count(*) FROM commerce.variant_selected_options WHERE product_id = $1)",
        )
        .bind(output.product_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(counts, (1, 2, 4, 1, 2));
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM commerce.products WHERE id = $1")
                .bind(output.product_id.as_uuid())
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        assert_eq!(status, "draft");

        sqlx::query("DELETE FROM commerce.stores WHERE id = $1")
            .bind(store_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM integration.idempotency_keys \
             WHERE scope = 'store' AND scope_id = $1",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = ANY($1)")
            .bind(vec![owner_user_id.as_uuid(), member_user_id.as_uuid()])
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
