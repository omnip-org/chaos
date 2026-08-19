use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, IdempotencyRequest, PriceListDetail, PriceListMutationSnapshot,
        PriceListReadItem, PriceReadItem, PricingManagementTransaction,
        PricingManagementUnitOfWork, PricingReadRepository,
    },
};
use chaos_domain::{
    CurrencyCode,
    catalog::ProductVariantId,
    merchant::StoreId,
    pricing::{PriceList, PriceListId, PriceListStatus},
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

#[derive(Clone)]
pub struct PostgresPricingManagementRepository {
    pool: PgPool,
}

impl PostgresPricingManagementRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_read(
        &self,
        actor: AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        set_context(&mut transaction, actor).await?;
        Ok(transaction)
    }
}

struct PostgresPricingManagementTransaction {
    transaction: Transaction<'static, Postgres>,
    store_id: StoreId,
    price_list_id: PriceListId,
}

type PriceListRow = (
    Uuid,
    String,
    String,
    String,
    bool,
    String,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    i64,
    OffsetDateTime,
    OffsetDateTime,
);

#[async_trait]
impl PricingReadRepository for PostgresPricingManagementRepository {
    async fn list_price_lists(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<PriceListId>,
        limit: u16,
    ) -> Result<Option<Vec<PriceListReadItem>>, ApplicationError> {
        let mut transaction = self.begin_read(actor).await?;
        let store_exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM merchant.stores WHERE id = $1)")
                .bind(store_id.as_uuid())
                .fetch_one(&mut *transaction)
                .await
                .map_err(database_error)?;
        if !store_exists {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, PriceListRow>(
            "SELECT price_list.id, price_list.code::text, price_list.name, \
                    price_list.currency::text, price_list.tax_inclusive, \
                    price_list.status::text, price_list.starts_at, price_list.ends_at, \
                    count(price.id), price_list.created_at, price_list.updated_at \
             FROM pricing.price_lists AS price_list \
             LEFT JOIN pricing.prices AS price \
              ON price.store_id = price_list.store_id \
              AND price.price_list_id = price_list.id \
             WHERE price_list.store_id = $1 \
               AND ($2::uuid IS NULL OR price_list.id > $2) \
             GROUP BY price_list.id \
             ORDER BY price_list.id ASC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(PriceListId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(read_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn get_price_list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        price_list_id: PriceListId,
    ) -> Result<Option<PriceListDetail>, ApplicationError> {
        let mut transaction = self.begin_read(actor).await?;
        let row = sqlx::query_as::<_, PriceListRow>(
            "SELECT price_list.id, price_list.code::text, price_list.name, \
                    price_list.currency::text, price_list.tax_inclusive, \
                    price_list.status::text, price_list.starts_at, price_list.ends_at, \
                    count(price.id), price_list.created_at, price_list.updated_at \
             FROM pricing.price_lists AS price_list \
             LEFT JOIN pricing.prices AS price \
              ON price.store_id = price_list.store_id \
              AND price.price_list_id = price_list.id \
             WHERE price_list.store_id = $1 AND price_list.id = $2 \
             GROUP BY price_list.id",
        )
        .bind(store_id.as_uuid())
        .bind(price_list_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let prices = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT product_variant_id, amount_minor FROM pricing.prices \
             WHERE store_id = $1 AND price_list_id = $2 \
             ORDER BY product_variant_id ASC",
        )
        .bind(store_id.as_uuid())
        .bind(price_list_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|(product_variant_id, amount_minor)| PriceReadItem {
            product_variant_id: ProductVariantId::from_uuid(product_variant_id),
            amount_minor,
        })
        .collect();
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(PriceListDetail {
            item: read_item(row)?,
            prices,
        }))
    }
}

#[async_trait]
impl PricingManagementUnitOfWork for PostgresPricingManagementRepository {
    async fn begin(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        price_list_id: PriceListId,
    ) -> Result<Box<dyn PricingManagementTransaction>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(&mut transaction, actor).await?;
        Ok(Box::new(PostgresPricingManagementTransaction {
            transaction,
            store_id,
            price_list_id,
        }))
    }
}

#[async_trait]
impl PricingManagementTransaction for PostgresPricingManagementTransaction {
    async fn reserve_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
    ) -> Result<Option<PriceListId>, ApplicationError> {
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
        body.pointer("/data/id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .map(PriceListId::from_uuid)
            .map(Some)
            .ok_or_else(|| {
                ApplicationError::Unexpected(anyhow::anyhow!(
                    "completed idempotency record has no price list mutation response"
                ))
            })
    }

    async fn load_for_update(
        &mut self,
    ) -> Result<Option<PriceListMutationSnapshot>, ApplicationError> {
        let status = sqlx::query_scalar::<_, String>(
            "SELECT status::text FROM pricing.price_lists \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.price_list_id.as_uuid())
        .fetch_optional(&mut *self.transaction)
        .await
        .map_err(database_error)?;
        let Some(status) = status else {
            return Ok(None);
        };
        let variant_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT product_variant_id FROM pricing.prices \
             WHERE store_id = $1 AND price_list_id = $2",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.price_list_id.as_uuid())
        .fetch_all(&mut *self.transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(ProductVariantId::from_uuid)
        .collect();
        Ok(Some(PriceListMutationSnapshot {
            status: parse_status(&status)?,
            priced_variant_ids: variant_ids,
        }))
    }

    async fn active_variant_ids(
        &mut self,
        variant_ids: &[ProductVariantId],
    ) -> Result<Vec<ProductVariantId>, ApplicationError> {
        variant_ids_with_status(
            &mut self.transaction,
            self.store_id,
            variant_ids,
            Some("active"),
        )
        .await
    }

    async fn store_variant_ids(
        &mut self,
        variant_ids: &[ProductVariantId],
    ) -> Result<Vec<ProductVariantId>, ApplicationError> {
        variant_ids_with_status(&mut self.transaction, self.store_id, variant_ids, None).await
    }

    async fn currency_is_enabled(
        &mut self,
        currency: CurrencyCode,
    ) -> Result<bool, ApplicationError> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM merchant.store_currencies \
             WHERE store_id = $1 AND currency = $2 AND enabled)",
        )
        .bind(self.store_id.as_uuid())
        .bind(currency.as_str())
        .fetch_one(&mut *self.transaction)
        .await
        .map_err(database_error)
    }

    async fn replace(&mut self, price_list: &PriceList) -> Result<(), ApplicationError> {
        let result = sqlx::query(
            "UPDATE pricing.price_lists SET code = $3, name = $4, currency = $5, \
                    tax_inclusive = $6, status = $7::pricing.price_list_status, \
                    starts_at = $8, ends_at = $9, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.price_list_id.as_uuid())
        .bind(price_list.code().as_str())
        .bind(price_list.name())
        .bind(price_list.currency().as_str())
        .bind(price_list.tax_inclusive())
        .bind(price_list.status().as_str())
        .bind(price_list.starts_at())
        .bind(price_list.ends_at())
        .execute(&mut *self.transaction)
        .await
        .map_err(map_write_error)?;
        if result.rows_affected() != 1 {
            return Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "locked price list disappeared during update"
            )));
        }
        sqlx::query(
            "DELETE FROM pricing.prices \
             WHERE store_id = $1 AND price_list_id = $2",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.price_list_id.as_uuid())
        .execute(&mut *self.transaction)
        .await
        .map_err(database_error)?;
        for price in price_list.prices() {
            sqlx::query(
                "INSERT INTO pricing.prices \
                 (id, store_id, price_list_id, product_variant_id, amount_minor) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(price.id().as_uuid())
            .bind(self.store_id.as_uuid())
            .bind(self.price_list_id.as_uuid())
            .bind(price.product_variant_id().as_uuid())
            .bind(price.amount().amount_minor())
            .execute(&mut *self.transaction)
            .await
            .map_err(map_write_error)?;
        }
        Ok(())
    }

    async fn set_status(&mut self, status: PriceListStatus) -> Result<(), ApplicationError> {
        let result = sqlx::query(
            "UPDATE pricing.price_lists SET status = $3::pricing.price_list_status, \
                    updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(self.store_id.as_uuid())
        .bind(self.price_list_id.as_uuid())
        .bind(status.as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(database_error)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(ApplicationError::Unexpected(anyhow::anyhow!(
                "locked price list disappeared during lifecycle mutation"
            )))
        }
    }

    async fn complete_mutation(
        &mut self,
        operation: &'static str,
        request: &IdempotencyRequest,
        price_list_id: PriceListId,
    ) -> Result<(), ApplicationError> {
        idempotency::complete(
            &mut self.transaction,
            &IdempotencyScope::Store(self.store_id.as_uuid()),
            operation,
            request,
            200,
            json!({ "data": { "id": price_list_id.as_uuid() } }),
        )
        .await
    }

    async fn commit(self: Box<Self>) -> Result<(), ApplicationError> {
        self.transaction.commit().await.map_err(database_error)
    }
}

fn read_item(row: PriceListRow) -> Result<PriceListReadItem, ApplicationError> {
    let (
        id,
        code,
        name,
        currency,
        tax_inclusive,
        status,
        starts_at,
        ends_at,
        price_count,
        created_at,
        updated_at,
    ) = row;
    Ok(PriceListReadItem {
        id: PriceListId::from_uuid(id),
        code,
        name,
        currency: CurrencyCode::parse(&currency).map_err(ApplicationError::from)?,
        tax_inclusive,
        status: parse_status(&status)?,
        starts_at,
        ends_at,
        price_count: u32::try_from(price_count).map_err(|_| {
            ApplicationError::Unexpected(anyhow::anyhow!("price count is out of range"))
        })?,
        created_at,
        updated_at,
    })
}

async fn set_context(
    transaction: &mut Transaction<'_, Postgres>,
    actor: AdminActor,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT set_config('app.user_id', $1, true)")
        .bind(actor.audit_user_id().as_uuid().to_string())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    sqlx::query("SELECT set_config('app.store_id', $1, true)")
        .bind(actor.store_id().as_uuid().to_string())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(())
}

async fn variant_ids_with_status(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
    variant_ids: &[ProductVariantId],
    status: Option<&str>,
) -> Result<Vec<ProductVariantId>, ApplicationError> {
    let ids = variant_ids
        .iter()
        .map(|id| id.as_uuid())
        .collect::<Vec<_>>();
    let rows = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM catalog.product_variants \
         WHERE store_id = $1 AND id = ANY($2) \
           AND ($3::text IS NULL OR status::text = $3)",
    )
    .bind(store_id.as_uuid())
    .bind(ids)
    .bind(status)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(rows.into_iter().map(ProductVariantId::from_uuid).collect())
}

fn parse_status(value: &str) -> Result<PriceListStatus, ApplicationError> {
    PriceListStatus::parse(value).ok_or_else(|| {
        ApplicationError::Unexpected(anyhow::anyhow!(
            "database contains an unknown price list status"
        ))
    })
}

fn map_write_error(error: sqlx::Error) -> ApplicationError {
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

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chaos_application::{
        merchant::MerchantQueries,
        ports::{AdminActor, IdempotencyRequest},
        pricing::{
            ChangePriceListStatusInput, CreatePriceInput, PricingManagement, UpdatePriceListInput,
        },
    };
    use chaos_domain::{
        catalog::{ProductId, ProductVariantId},
        identity::UserId,
    };
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn manages_price_lists_with_idempotency_authorization_and_store_isolation() {
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
        let variant_id = ProductVariantId::new();
        let price_list_id = PriceListId::new();
        let suffix = Uuid::now_v7().simple().to_string();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_id.as_uuid())
            .bind(format!("pricing-management-owner-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        for (id, code) in [
            (store_id, "pricing-admin"),
            (other_store_id, "pricing-other"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.stores (id, code, name) \
                 VALUES ($1, $2, 'Pricing Store')",
            )
            .bind(id.as_uuid())
            .bind(format!("{code}-{suffix}"))
            .execute(&owner_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO merchant.store_currencies \
                 (store_id, currency) VALUES ($1, 'USD')",
            )
            .bind(id.as_uuid())
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
        sqlx::query(
            "INSERT INTO catalog.products \
             (id, store_id, handle, title, status) \
             VALUES ($1, $2, 'managed-product', 'Managed Product', 'active')",
        )
        .bind(product_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, store_id, product_id, title, status) \
             VALUES ($1, $2, $3, 'Default', 'active')",
        )
        .bind(variant_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(product_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.price_lists \
             (id, store_id, code, name, currency) \
             VALUES ($1, $2, 'retail', 'Retail', 'USD')",
        )
        .bind(price_list_id.as_uuid())
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO pricing.prices \
             (id, store_id, price_list_id, product_variant_id, amount_minor) \
             VALUES ($1, $2, $3, $4, 2500)",
        )
        .bind(Uuid::now_v7())
        .bind(store_id.as_uuid())
        .bind(price_list_id.as_uuid())
        .bind(variant_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let queries = MerchantQueries::new(Arc::new(
            crate::repositories::PostgresMerchantReadRepository::new(runtime_pool.clone()),
        ));
        let owner = queries.authorize(owner_id, store_id).await.unwrap();
        let repository = Arc::new(PostgresPricingManagementRepository::new(runtime_pool));
        let service = PricingManagement::new(repository.clone(), repository);

        let page = service
            .list(AdminActor::Store(owner), store_id, None, 20)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let detail = service
            .get(AdminActor::Store(owner), store_id, price_list_id)
            .await
            .unwrap();
        assert_eq!(detail.prices[0].amount_minor, 2500);
        assert!(
            service
                .get(AdminActor::Store(owner), other_store_id, price_list_id)
                .await
                .is_err()
        );

        let update_input = || UpdatePriceListInput {
            actor: AdminActor::Store(owner),
            store_id,
            price_list_id,
            code: "retail-us".into(),
            name: "US Retail".into(),
            currency: "USD".into(),
            tax_inclusive: true,
            starts_at: None,
            ends_at: None,
            prices: vec![CreatePriceInput {
                product_variant_id: variant_id,
                amount_minor: 3000,
            }],
            idempotency: IdempotencyRequest {
                key: format!("update-{suffix}"),
                request_fingerprint: [61; 32],
            },
        };
        service.update(update_input()).await.unwrap();
        service.update(update_input()).await.unwrap();
        let updated = service
            .get(AdminActor::Store(owner), store_id, price_list_id)
            .await
            .unwrap();
        assert_eq!(updated.item.code, "retail-us");
        assert!(updated.item.tax_inclusive);
        assert_eq!(updated.prices[0].amount_minor, 3000);

        service
            .activate(ChangePriceListStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
                price_list_id,
                idempotency: IdempotencyRequest {
                    key: format!("activate-{suffix}"),
                    request_fingerprint: [62; 32],
                },
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .get(AdminActor::Store(owner), store_id, price_list_id)
                .await
                .unwrap()
                .item
                .status,
            PriceListStatus::Active
        );
        service
            .archive(ChangePriceListStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
                price_list_id,
                idempotency: IdempotencyRequest {
                    key: format!("archive-{suffix}"),
                    request_fingerprint: [64; 32],
                },
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .get(AdminActor::Store(owner), store_id, price_list_id)
                .await
                .unwrap()
                .item
                .status,
            PriceListStatus::Archived
        );
    }
}
