use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, PricingProvisioningTransaction,
        PricingProvisioningUnitOfWork,
    },
};
use chaos_domain::{
    CurrencyCode,
    catalog::ProductVariantId,
    pricing::{PriceList, PriceListId},
    store::StoreId,
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::repositories::shared::idempotency::{self, IdempotencyScope};

const CREATE_PRICE_LIST_OPERATION: &str = "price_lists.create.v1";

#[derive(Clone)]
pub struct PostgresPricingProvisioningUnitOfWork {
    pool: PgPool,
}

impl PostgresPricingProvisioningUnitOfWork {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

struct PostgresPricingProvisioningTransaction {
    transaction: Transaction<'static, Postgres>,
    store_id: StoreId,
}

#[async_trait]
impl PricingProvisioningUnitOfWork for PostgresPricingProvisioningUnitOfWork {
    async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Box<dyn PricingProvisioningTransaction>, ApplicationError> {
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
        Ok(Box::new(PostgresPricingProvisioningTransaction {
            transaction,
            store_id,
        }))
    }
}

#[async_trait]
impl PricingProvisioningTransaction for PostgresPricingProvisioningTransaction {
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

    async fn require_enabled_currency(
        &mut self,
        currency: CurrencyCode,
    ) -> Result<(), ApplicationError> {
        let enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS (\
                SELECT 1 FROM commerce.store_currencies \
                WHERE store_id = $1 \
                  AND currency = $2 AND enabled\
             )",
        )
        .bind(self.store_id.as_uuid())
        .bind(currency.as_str())
        .fetch_one(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        if enabled {
            Ok(())
        } else {
            Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "currency",
                    reason: "must be enabled for the Store".into(),
                }],
            })
        }
    }

    async fn reserve_price_list_creation(
        &mut self,
        request: &IdempotencyRequest,
    ) -> Result<Option<PriceListId>, ApplicationError> {
        let Some(body) = idempotency::reserve(
            &mut self.transaction,
            &IdempotencyScope::Store(self.store_id.as_uuid()),
            CREATE_PRICE_LIST_OPERATION,
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
            .map(PriceListId::from_uuid)
            .map(Some)
            .ok_or_else(|| {
                ApplicationError::Unexpected(anyhow::anyhow!(
                    "completed idempotency record has no price list response"
                ))
            })
    }

    async fn active_variant_ids(
        &mut self,
        variant_ids: &[ProductVariantId],
    ) -> Result<Vec<ProductVariantId>, ApplicationError> {
        let ids = variant_ids
            .iter()
            .map(|id| id.as_uuid())
            .collect::<Vec<_>>();
        let rows = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM commerce.product_variants \
             WHERE store_id = $1 \
               AND id = ANY($2) AND status = 'active'",
        )
        .bind(self.store_id.as_uuid())
        .bind(ids)
        .fetch_all(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        Ok(rows.into_iter().map(ProductVariantId::from_uuid).collect())
    }

    async fn store_variant_ids(
        &mut self,
        variant_ids: &[ProductVariantId],
    ) -> Result<Vec<ProductVariantId>, ApplicationError> {
        let ids = variant_ids
            .iter()
            .map(|id| id.as_uuid())
            .collect::<Vec<_>>();
        let rows = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM commerce.product_variants \
             WHERE store_id = $1 AND id = ANY($2)",
        )
        .bind(self.store_id.as_uuid())
        .bind(ids)
        .fetch_all(&mut *self.transaction)
        .await
        .map_err(unexpected_database_error)?;
        Ok(rows.into_iter().map(ProductVariantId::from_uuid).collect())
    }

    async fn insert_price_list(&mut self, price_list: &PriceList) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO commerce.price_lists \
             (id, store_id, code, name, currency, status, starts_at, ends_at) \
             VALUES ($1, $2, $3, $4, $5, $6::commerce.price_list_status, $7, $8)",
        )
        .bind(price_list.id().as_uuid())
        .bind(price_list.store_id().as_uuid())
        .bind(price_list.code().as_str())
        .bind(price_list.name())
        .bind(price_list.currency().as_str())
        .bind(price_list.status().as_str())
        .bind(price_list.starts_at())
        .bind(price_list.ends_at())
        .execute(&mut *self.transaction)
        .await
        .map_err(map_pricing_write_error)?;
        for price in price_list.prices() {
            sqlx::query(
                "INSERT INTO commerce.prices \
                 (id, store_id, price_list_id, product_variant_id, \
                  amount_minor) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(price.id().as_uuid())
            .bind(price_list.store_id().as_uuid())
            .bind(price_list.id().as_uuid())
            .bind(price.product_variant_id().as_uuid())
            .bind(price.amount().amount_minor())
            .execute(&mut *self.transaction)
            .await
            .map_err(map_pricing_write_error)?;
        }
        Ok(())
    }

    async fn complete_price_list_creation(
        &mut self,
        request: &IdempotencyRequest,
        price_list_id: PriceListId,
    ) -> Result<(), ApplicationError> {
        idempotency::complete(
            &mut self.transaction,
            &IdempotencyScope::Store(self.store_id.as_uuid()),
            CREATE_PRICE_LIST_OPERATION,
            request,
            201,
            json!({ "data": { "id": price_list_id.as_uuid() } }),
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

fn map_pricing_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error
        && database_error.constraint() == Some("price_lists_store_id_code_key")
    {
        return ApplicationError::Conflict {
            code: "price_list_code_taken",
            message: "the price list code is already in use for this Store",
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
        ports::AdminActor,
        pricing::{CreatePriceInput, CreatePriceList, CreatePriceListInput},
        store::StoreQueries,
    };
    use chaos_domain::{catalog::ProductId, identity::UserId};
    use sqlx::postgres::PgPoolOptions;

    use crate::repositories::PostgresStoreReadRepository;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn creates_an_isolated_active_price_list_atomically() {
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
        let other_product_id = ProductId::new();
        let variant_id = ProductVariantId::new();
        let other_variant_id = ProductVariantId::new();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_id.as_uuid())
            .bind(format!("pricing-owner-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        for (id, code) in [
            (store_id, format!("pricing-{suffix}")),
            (other_store_id, format!("pricing-other-{suffix}")),
        ] {
            sqlx::query(
                "INSERT INTO commerce.stores (id, code, name, status) \
                 VALUES ($1, $2, 'Pricing Store', 'active')",
            )
            .bind(id.as_uuid())
            .bind(code)
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO commerce.store_currencies \
                 (store_id, currency, enabled) \
                 VALUES ($1, 'USD', true)",
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
        .bind(owner_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.store_currencies \
             (store_id, currency, enabled) \
             VALUES ($1, 'EUR', false)",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        for (product, variant, store, handle, sku) in [
            (
                product_id,
                variant_id,
                store_id,
                "priced-product",
                "PRICED-SKU",
            ),
            (
                other_product_id,
                other_variant_id,
                other_store_id,
                "other-product",
                "OTHER-SKU",
            ),
        ] {
            sqlx::query(
                "INSERT INTO commerce.products \
                 (id, store_id, handle, title, status) \
                 VALUES ($1, $2, $3, 'Priced Product', 'active')",
            )
            .bind(product.as_uuid())
            .bind(store.as_uuid())
            .bind(handle)
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO commerce.product_variants \
                 (id, store_id, product_id, title, sku) \
                 VALUES ($1, $2, $3, 'Default', $4)",
            )
            .bind(variant.as_uuid())
            .bind(store.as_uuid())
            .bind(product.as_uuid())
            .bind(sku)
            .execute(&owner_pool)
            .await
            .unwrap();
        }

        let directory = StoreQueries::new(Arc::new(PostgresStoreReadRepository::new(
            runtime_pool.clone(),
        )));
        let owner = directory.authorize(owner_id, store_id).await.unwrap();
        let service = CreatePriceList::new(Arc::new(PostgresPricingProvisioningUnitOfWork::new(
            runtime_pool.clone(),
        )));
        let key = format!("price-list-{suffix}");
        let input = |actor: chaos_application::store::StoreActor,
                     currency: &str,
                     variant,
                     fingerprint,
                     key: String| CreatePriceListInput {
            actor: AdminActor::Store(actor),
            store_id,
            code: "us-retail".into(),
            name: "US Retail".into(),
            currency: currency.into(),
            starts_at: None,
            ends_at: None,
            activate: true,
            prices: vec![CreatePriceInput {
                product_variant_id: variant,
                amount_minor: 2_500,
            }],
            idempotency: IdempotencyRequest {
                key,
                request_fingerprint: fingerprint,
            },
        };
        assert!(matches!(
            service
                .execute(input(
                    owner,
                    "EUR",
                    variant_id,
                    [51; 32],
                    format!("disabled-{suffix}"),
                ))
                .await,
            Err(ApplicationError::Validation { .. })
        ));
        assert!(matches!(
            service
                .execute(input(
                    owner,
                    "USD",
                    other_variant_id,
                    [52; 32],
                    format!("cross-store-{suffix}"),
                ))
                .await,
            Err(ApplicationError::Validation { .. })
        ));
        let created = service
            .execute(input(owner, "USD", variant_id, [53; 32], key.clone()))
            .await
            .unwrap();
        let replay = service
            .execute(input(owner, "USD", variant_id, [53; 32], key))
            .await
            .unwrap();
        assert_eq!(created.price_list_id, replay.price_list_id);

        let stored: (String, String, i64) = sqlx::query_as(
            "SELECT price_list.status::text, price_list.currency::text, price.amount_minor \
             FROM commerce.price_lists AS price_list \
             INNER JOIN commerce.prices AS price \
              ON price.store_id = price_list.store_id \
              AND price.price_list_id = price_list.id \
             WHERE price_list.id = $1",
        )
        .bind(created.price_list_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(stored, ("active".into(), "USD".into(), 2_500));

        let mut isolated = runtime_pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(other_store_id.as_uuid().to_string())
            .execute(&mut *isolated)
            .await
            .unwrap();
        let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM commerce.price_lists")
            .fetch_one(&mut *isolated)
            .await
            .unwrap();
        assert_eq!(visible, 0);

        sqlx::query("DELETE FROM commerce.stores WHERE id = ANY($1)")
            .bind(vec![store_id.as_uuid(), other_store_id.as_uuid()])
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
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(owner_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
