use crate::{ApplicationError, contracts::AdminActor, error::database_error};
use chaos_domain::{CurrencyCode, catalog::ProductVariantId, pricing::PriceList, store::StoreId};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresPricingProvisioningRepository {
    pool: PgPool,
}

impl PostgresPricingProvisioningRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

pub(crate) struct PostgresPricingProvisioningTransaction {
    transaction: Transaction<'static, Postgres>,
    store_id: StoreId,
}

impl PostgresPricingProvisioningRepository {
    pub(crate) async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<PostgresPricingProvisioningTransaction, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_admin_context(
            &mut transaction,
            actor.audit_user_id(),
            store_id,
        )
        .await
        .map_err(database_error)?;
        Ok(PostgresPricingProvisioningTransaction {
            transaction,
            store_id,
        })
    }
}

impl PostgresPricingProvisioningTransaction {
    pub(crate) async fn require_writable_store(&mut self) -> Result<(), ApplicationError> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM commerce.stores WHERE id = $1 AND status = 'active')",
        )
        .bind(self.store_id.as_uuid())
        .fetch_one(&mut *self.transaction)
        .await
        .map_err(database_error)?;
        if exists {
            Ok(())
        } else {
            Err(ApplicationError::NotFound {
                resource: "store",
                id: self.store_id.as_uuid().to_string(),
            })
        }
    }

    pub(crate) async fn require_store_currency(
        &mut self,
        currency: CurrencyCode,
    ) -> Result<(), ApplicationError> {
        let matches_store: bool =
            sqlx::query_scalar("SELECT currency = $2 FROM commerce.stores WHERE id = $1")
                .bind(self.store_id.as_uuid())
                .bind(currency.as_str())
                .fetch_one(&mut *self.transaction)
                .await
                .map_err(database_error)?;
        if matches_store {
            Ok(())
        } else {
            Err(ApplicationError::Validation {
                violations: vec![chaos_domain::FieldViolation {
                    field: "currency",
                    reason: "must match the Store currency".into(),
                }],
            })
        }
    }

    pub(crate) async fn active_variant_ids(
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
        .map_err(database_error)?;
        Ok(rows.into_iter().map(ProductVariantId::from_uuid).collect())
    }

    pub(crate) async fn store_variant_ids(
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
        .map_err(database_error)?;
        Ok(rows.into_iter().map(ProductVariantId::from_uuid).collect())
    }

    pub(crate) async fn insert_price_list(
        &mut self,
        price_list: &PriceList,
    ) -> Result<(), ApplicationError> {
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

    pub(crate) async fn commit(self) -> Result<(), ApplicationError> {
        self.transaction.commit().await.map_err(database_error)
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
    database_error(error)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        contracts::AdminActor,
        pricing::{CreatePriceInput, CreatePriceList, CreatePriceListInput},
        store::StoreQueries,
    };
    use chaos_domain::{catalog::ProductId, identity::UserId};
    use sqlx::postgres::PgPoolOptions;

    use crate::adapters::postgres::PostgresStoreReadRepository;

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
        let service = CreatePriceList::new(Arc::new(PostgresPricingProvisioningRepository::new(
            runtime_pool.clone(),
        )));
        let input =
            |actor: crate::store::StoreActor, currency: &str, variant| CreatePriceListInput {
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
            };
        assert!(matches!(
            service.execute(input(owner, "EUR", variant_id,)).await,
            Err(ApplicationError::Validation { .. })
        ));
        assert!(matches!(
            service
                .execute(input(owner, "USD", other_variant_id,))
                .await,
            Err(ApplicationError::Validation { .. })
        ));
        let created = service
            .execute(input(owner, "USD", variant_id))
            .await
            .unwrap();

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
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(owner_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
