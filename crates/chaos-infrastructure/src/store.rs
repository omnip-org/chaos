use anyhow::Context;
use chaos_domain::store::StoreId;
use sqlx::{PgPool, Postgres, Transaction};

#[cfg(test)]
use sqlx::PgConnection;

pub struct StoreTransaction<'a> {
    inner: Transaction<'a, Postgres>,
    store_id: StoreId,
}

impl<'a> StoreTransaction<'a> {
    pub(crate) async fn begin(pool: &'a PgPool, store_id: StoreId) -> anyhow::Result<Self> {
        let mut inner = pool
            .begin()
            .await
            .context("failed to begin store transaction")?;
        sqlx::query("SELECT set_config('app.store_id', $1, true)")
            .bind(store_id.as_uuid().to_string())
            .execute(&mut *inner)
            .await
            .context("failed to establish PostgreSQL store context")?;

        Ok(Self { inner, store_id })
    }

    pub const fn store_id(&self) -> StoreId {
        self.store_id
    }

    #[cfg(test)]
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        &mut self.inner
    }

    pub async fn commit(self) -> anyhow::Result<()> {
        self.inner
            .commit()
            .await
            .context("failed to commit store transaction")
    }

    pub async fn rollback(self) -> anyhow::Result<()> {
        self.inner
            .rollback()
            .await
            .context("failed to roll back store transaction")
    }
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn rls_hides_other_stores_rows() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let public_business_tables: i64 = sqlx::query_scalar(
            "SELECT count(*) \
             FROM pg_catalog.pg_tables \
             WHERE schemaname = 'public' AND tablename <> '_sqlx_migrations'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(public_business_tables, 0);

        let store_a = Uuid::now_v7();
        let store_b = Uuid::now_v7();
        let channel_a = Uuid::now_v7();
        let channel_b = Uuid::now_v7();
        let product_a = Uuid::now_v7();
        let product_b = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let key_a = Uuid::now_v7();
        let key_b = Uuid::now_v7();
        let tax_rule_a = Uuid::now_v7();
        let tax_rule_b = Uuid::now_v7();
        let promotion_a = Uuid::now_v7();
        let promotion_b = Uuid::now_v7();
        let customer_a = Uuid::now_v7();
        let customer_b = Uuid::now_v7();
        let customer_address_a = Uuid::now_v7();
        let customer_address_b = Uuid::now_v7();
        let shopper_a = Uuid::now_v7();
        let shopper_b = Uuid::now_v7();
        let provider_account_a = Uuid::now_v7();
        let provider_account_b = Uuid::now_v7();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id)
            .bind(format!("rls-api-keys-{}@example.com", user_id.simple()))
            .execute(&pool)
            .await
            .unwrap();

        for (store_id, code) in [(store_a, "store-a"), (store_b, "store-b")] {
            sqlx::query(
                "INSERT INTO commerce.stores (id, code, name, default_currency, status) \
                 VALUES ($1, $2, $3, 'USD', 'active')",
            )
            .bind(store_id)
            .bind(code)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO commerce.store_memberships (store_id, user_id, role) \
                 VALUES ($1, $2, 'owner')",
            )
            .bind(store_id)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO commerce.store_currencies (store_id, currency) VALUES ($1, 'USD')",
            )
            .bind(store_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (channel_id, store_id, code) in
            [(channel_a, store_a, "web-a"), (channel_b, store_b, "web-b")]
        {
            sqlx::query(
                "INSERT INTO commerce.sales_channels \
                 (id, store_id, code, name, kind, is_default) \
                 VALUES ($1, $2, $3, $3, 'web', true)",
            )
            .bind(channel_id)
            .bind(store_id)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (tax_rule_id, store_id, code) in [
            (tax_rule_a, store_a, "tax-a"),
            (tax_rule_b, store_b, "tax-b"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.tax_rules \
                 (id, store_id, code, name, country_code, rate_basis_points) \
                 VALUES ($1, $2, $3, $3, 'US', 0)",
            )
            .bind(tax_rule_id)
            .bind(store_id)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (promotion_id, store_id, handle) in [
            (promotion_a, store_a, "promotion-a"),
            (promotion_b, store_b, "promotion-b"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.promotions \
                 (id, store_id, handle, name, trigger, value_kind, \
                  rate_basis_points, currency) \
                 VALUES ($1, $2, $3, $3, 'automatic', 'percentage', 1000, 'USD')",
            )
            .bind(promotion_id)
            .bind(store_id)
            .bind(handle)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (product_id, store_id, handle) in [
            (product_a, store_a, "product-a"),
            (product_b, store_b, "product-b"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.products (id, store_id, handle, title) \
                 VALUES ($1, $2, $3, $3)",
            )
            .bind(product_id)
            .bind(store_id)
            .bind(handle)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (key_id, store_id, identifier) in [
            (key_a, store_a, "RlsKeyAccountA01"),
            (key_b, store_b, "RlsKeyAccountB02"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.publishable_keys \
                 (id, store_id, key_identifier, secret_digest, \
                  display_suffix, name, created_by_user_id) \
                 VALUES ($1, $2, $3, $4, 'abcd', 'RLS test key', $5)",
            )
            .bind(key_id)
            .bind(store_id)
            .bind(identifier)
            .bind(vec![7_u8; 32])
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (customer_id, address_id, shopper_id, store_id, channel_id, label) in [
            (
                customer_a,
                customer_address_a,
                shopper_a,
                store_a,
                channel_a,
                "Home A",
            ),
            (
                customer_b,
                customer_address_b,
                shopper_b,
                store_b,
                channel_b,
                "Home B",
            ),
        ] {
            sqlx::query(
                "INSERT INTO commerce.customers (id, store_id, user_id, email) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(customer_id)
            .bind(store_id)
            .bind(user_id)
            .bind(format!("{customer_id}@example.com"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO commerce.customer_addresses \
                 (id, store_id, customer_id, label, full_name, \
                  address_line1, locality, country_code) \
                 VALUES ($1, $2, $3, $4, 'RLS Customer', '1 Main', 'Town', 'US')",
            )
            .bind(address_id)
            .bind(store_id)
            .bind(customer_id)
            .bind(label)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO commerce.customer_shopper_links \
                 (store_id, customer_id, shopper_id, sales_channel_id) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(store_id)
            .bind(customer_id)
            .bind(shopper_id)
            .bind(channel_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (id, store_id, external_reference) in [
            (provider_account_a, store_a, "rls-provider-a"),
            (provider_account_b, store_b, "rls-provider-b"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.provider_accounts \
                 (id, store_id, provider, external_account_reference) \
                 VALUES ($1, $2, 'testpay', $3)",
            )
            .bind(id)
            .bind(store_id)
            .bind(external_reference)
            .execute(&pool)
            .await
            .unwrap();
        }

        let mut transaction = StoreTransaction::begin(&pool, StoreId::from_uuid(store_a))
            .await
            .unwrap();
        sqlx::query("SET LOCAL ROLE chaos_runtime")
            .execute(transaction.connection())
            .await
            .unwrap();
        let visible_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.stores ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_key_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.publishable_keys ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_channel_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.sales_channels ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_product_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.products ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_tax_rule_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.tax_rules ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_promotion_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.promotions ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_customer_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.customers ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_customer_address_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.customer_addresses ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_shopper_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT shopper_id FROM commerce.customer_shopper_links ORDER BY shopper_id",
        )
        .fetch_all(transaction.connection())
        .await
        .unwrap();
        let visible_provider_account_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.provider_accounts ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        transaction.rollback().await.unwrap();

        assert_eq!(visible_ids, vec![store_a]);
        assert_eq!(visible_key_ids, vec![key_a]);
        assert_eq!(visible_channel_ids, vec![channel_a]);
        assert_eq!(visible_product_ids, vec![product_a]);
        assert_eq!(visible_tax_rule_ids, vec![tax_rule_a]);
        assert_eq!(visible_promotion_ids, vec![promotion_a]);
        assert_eq!(visible_customer_ids, vec![customer_a]);
        assert_eq!(visible_customer_address_ids, vec![customer_address_a]);
        assert_eq!(visible_shopper_ids, vec![shopper_a]);
        assert_eq!(visible_provider_account_ids, vec![provider_account_a]);

        sqlx::query("DELETE FROM commerce.provider_accounts WHERE store_id = ANY($1)")
            .bind(vec![store_a, store_b])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM commerce.stores WHERE id = ANY($1)")
            .bind(vec![store_a, store_b])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn catalog_rejects_cross_product_option_selections() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let store_id = Uuid::now_v7();
        let product_a = Uuid::now_v7();
        let product_b = Uuid::now_v7();
        let option_a = Uuid::now_v7();
        let option_value_a = Uuid::now_v7();
        let variant_b = Uuid::now_v7();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query(
            "INSERT INTO commerce.stores (id, code, name) \
             VALUES ($1, $2, 'Catalog Test')",
        )
        .bind(store_id)
        .bind(format!("catalog-test-{suffix}"))
        .execute(&pool)
        .await
        .unwrap();
        for (product_id, handle) in [(product_a, "product-a"), (product_b, "product-b")] {
            sqlx::query(
                "INSERT INTO commerce.products (id, store_id, handle, title) \
                 VALUES ($1, $2, $3, $3)",
            )
            .bind(product_id)
            .bind(store_id)
            .bind(handle)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO commerce.product_options \
             (id, store_id, product_id, name, position) \
             VALUES ($1, $2, $3, 'Color', 0)",
        )
        .bind(option_a)
        .bind(store_id)
        .bind(product_a)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.product_option_values \
             (id, store_id, product_id, option_id, value, position) \
             VALUES ($1, $2, $3, $4, 'Blue', 0)",
        )
        .bind(option_value_a)
        .bind(store_id)
        .bind(product_a)
        .bind(option_a)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.product_variants \
             (id, store_id, product_id, title) \
             VALUES ($1, $2, $3, 'Default')",
        )
        .bind(variant_b)
        .bind(store_id)
        .bind(product_b)
        .execute(&pool)
        .await
        .unwrap();

        let cross_product = sqlx::query(
            "INSERT INTO commerce.variant_selected_options \
             (store_id, product_id, variant_id, \
              option_id, option_value_id) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(store_id)
        .bind(product_b)
        .bind(variant_b)
        .bind(option_a)
        .bind(option_value_a)
        .execute(&pool)
        .await;
        assert!(cross_product.is_err());

        sqlx::query("DELETE FROM commerce.stores WHERE id = $1")
            .bind(store_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
