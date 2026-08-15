use anyhow::Context;
use chaos_domain::merchant::MerchantAccountId;
use sqlx::{PgPool, Postgres, Transaction};

#[cfg(test)]
use sqlx::PgConnection;

pub struct MerchantAccountTransaction<'a> {
    inner: Transaction<'a, Postgres>,
    merchant_account_id: MerchantAccountId,
}

impl<'a> MerchantAccountTransaction<'a> {
    pub(crate) async fn begin(
        pool: &'a PgPool,
        merchant_account_id: MerchantAccountId,
    ) -> anyhow::Result<Self> {
        let mut inner = pool
            .begin()
            .await
            .context("failed to begin merchant account transaction")?;
        sqlx::query("SELECT set_config('app.merchant_account_id', $1, true)")
            .bind(merchant_account_id.as_uuid().to_string())
            .execute(&mut *inner)
            .await
            .context("failed to establish PostgreSQL merchant account context")?;

        Ok(Self {
            inner,
            merchant_account_id,
        })
    }

    pub const fn merchant_account_id(&self) -> MerchantAccountId {
        self.merchant_account_id
    }

    #[cfg(test)]
    pub(crate) fn connection(&mut self) -> &mut PgConnection {
        &mut self.inner
    }

    pub async fn commit(self) -> anyhow::Result<()> {
        self.inner
            .commit()
            .await
            .context("failed to commit merchant account transaction")
    }

    pub async fn rollback(self) -> anyhow::Result<()> {
        self.inner
            .rollback()
            .await
            .context("failed to roll back merchant account transaction")
    }
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn rls_hides_other_merchant_accounts_rows() {
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

        let account_a = MerchantAccountId::new();
        let account_b = MerchantAccountId::new();
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

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id)
            .bind(format!("rls-api-keys-{}@example.com", user_id.simple()))
            .execute(&pool)
            .await
            .unwrap();

        for (account_id, slug) in [
            (account_a, "rls-test-account-a"),
            (account_b, "rls-test-account-b"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
                 VALUES ($1, $2, $3)",
            )
            .bind(account_id.as_uuid())
            .bind(format!("{slug}-{}", Uuid::now_v7().simple()))
            .bind(slug)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (store_id, account_id, code) in [
            (store_a, account_a, "store-a"),
            (store_b, account_b, "store-b"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.stores \
                 (id, merchant_account_id, code, name, default_currency) \
                 VALUES ($1, $2, $3, $4, 'USD')",
            )
            .bind(store_id)
            .bind(account_id.as_uuid())
            .bind(code)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO merchant.store_currencies \
                 (merchant_account_id, store_id, currency) VALUES ($1, $2, 'USD')",
            )
            .bind(account_id.as_uuid())
            .bind(store_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (channel_id, store_id, account_id, code) in [
            (channel_a, store_a, account_a, "web-a"),
            (channel_b, store_b, account_b, "web-b"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.sales_channels \
                 (id, merchant_account_id, store_id, code, name, kind, is_default) \
                 VALUES ($1, $2, $3, $4, $4, 'web', true)",
            )
            .bind(channel_id)
            .bind(account_id.as_uuid())
            .bind(store_id)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (tax_rule_id, store_id, account_id, code) in [
            (tax_rule_a, store_a, account_a, "tax-a"),
            (tax_rule_b, store_b, account_b, "tax-b"),
        ] {
            sqlx::query(
                "INSERT INTO pricing.tax_rules \
                 (id, merchant_account_id, store_id, code, name, country_code, rate_basis_points) \
                 VALUES ($1, $2, $3, $4, $4, 'US', 0)",
            )
            .bind(tax_rule_id)
            .bind(account_id.as_uuid())
            .bind(store_id)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (promotion_id, store_id, account_id, handle) in [
            (promotion_a, store_a, account_a, "promotion-a"),
            (promotion_b, store_b, account_b, "promotion-b"),
        ] {
            sqlx::query(
                "INSERT INTO pricing.promotions \
                 (id, merchant_account_id, store_id, handle, name, trigger, value_kind, \
                  rate_basis_points, currency) \
                 VALUES ($1, $2, $3, $4, $4, 'automatic', 'percentage', 1000, 'USD')",
            )
            .bind(promotion_id)
            .bind(account_id.as_uuid())
            .bind(store_id)
            .bind(handle)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (product_id, store_id, account_id, handle) in [
            (product_a, store_a, account_a, "product-a"),
            (product_b, store_b, account_b, "product-b"),
        ] {
            sqlx::query(
                "INSERT INTO catalog.products \
                 (id, merchant_account_id, store_id, handle, title) \
                 VALUES ($1, $2, $3, $4, $4)",
            )
            .bind(product_id)
            .bind(account_id.as_uuid())
            .bind(store_id)
            .bind(handle)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (key_id, store_id, account_id, identifier) in [
            (key_a, store_a, account_a, "RlsKeyAccountA01"),
            (key_b, store_b, account_b, "RlsKeyAccountB02"),
        ] {
            sqlx::query(
                "INSERT INTO merchant.api_keys \
                 (id, merchant_account_id, store_id, key_identifier, secret_digest, \
                  display_suffix, name, class, mode, created_by_user_id) \
                 VALUES ($1, $2, $3, $4, $5, 'abcd', 'RLS test key', \
                         'secret', 'test', $6)",
            )
            .bind(key_id)
            .bind(account_id.as_uuid())
            .bind(store_id)
            .bind(identifier)
            .bind(vec![7_u8; 32])
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO merchant.api_key_scopes \
                 (merchant_account_id, api_key_id, scope) \
                 VALUES ($1, $2, 'mcp:tools')",
            )
            .bind(account_id.as_uuid())
            .bind(key_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (customer_id, address_id, shopper_id, store_id, channel_id, account_id, label) in [
            (
                customer_a,
                customer_address_a,
                shopper_a,
                store_a,
                channel_a,
                account_a,
                "Home A",
            ),
            (
                customer_b,
                customer_address_b,
                shopper_b,
                store_b,
                channel_b,
                account_b,
                "Home B",
            ),
        ] {
            sqlx::query(
                "INSERT INTO sales.customers \
                 (id, merchant_account_id, store_id, user_id, email) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(customer_id)
            .bind(account_id.as_uuid())
            .bind(store_id)
            .bind(user_id)
            .bind(format!("{customer_id}@example.com"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO sales.customer_addresses \
                 (id, merchant_account_id, store_id, customer_id, label, full_name, \
                  address_line1, locality, country_code) \
                 VALUES ($1, $2, $3, $4, $5, 'RLS Customer', '1 Main', 'Town', 'US')",
            )
            .bind(address_id)
            .bind(account_id.as_uuid())
            .bind(store_id)
            .bind(customer_id)
            .bind(label)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO sales.customer_shopper_links \
                 (merchant_account_id, store_id, customer_id, shopper_id, sales_channel_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(account_id.as_uuid())
            .bind(store_id)
            .bind(customer_id)
            .bind(shopper_id)
            .bind(channel_id)
            .execute(&pool)
            .await
            .unwrap();
        }

        let mut transaction = MerchantAccountTransaction::begin(&pool, account_a)
            .await
            .unwrap();
        sqlx::query("SET LOCAL ROLE chaos_runtime")
            .execute(transaction.connection())
            .await
            .unwrap();
        let visible_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM merchant.stores ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_key_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM merchant.api_keys ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_channel_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM merchant.sales_channels ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_product_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM catalog.products ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_tax_rule_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM pricing.tax_rules ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_promotion_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM pricing.promotions ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_scope_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM merchant.api_key_scopes WHERE scope = 'mcp:tools'",
        )
        .fetch_one(transaction.connection())
        .await
        .unwrap();
        let visible_customer_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM sales.customers ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_customer_address_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM sales.customer_addresses ORDER BY id")
                .fetch_all(transaction.connection())
                .await
                .unwrap();
        let visible_shopper_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT shopper_id FROM sales.customer_shopper_links ORDER BY shopper_id",
        )
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
        assert_eq!(visible_scope_count, 1);
        assert_eq!(visible_customer_ids, vec![customer_a]);
        assert_eq!(visible_customer_address_ids, vec![customer_address_a]);
        assert_eq!(visible_shopper_ids, vec![shopper_a]);

        sqlx::query("DELETE FROM merchant.stores WHERE merchant_account_id = ANY($1)")
            .bind(vec![account_a.as_uuid(), account_b.as_uuid()])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM merchant.merchant_accounts WHERE id = ANY($1)")
            .bind(vec![account_a.as_uuid(), account_b.as_uuid()])
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
        let account_id = Uuid::now_v7();
        let store_id = Uuid::now_v7();
        let product_a = Uuid::now_v7();
        let product_b = Uuid::now_v7();
        let option_a = Uuid::now_v7();
        let option_value_a = Uuid::now_v7();
        let variant_b = Uuid::now_v7();
        let suffix = Uuid::now_v7().simple().to_string();

        sqlx::query(
            "INSERT INTO merchant.merchant_accounts (id, slug, display_name) \
             VALUES ($1, $2, 'Catalog Constraint Test')",
        )
        .bind(account_id)
        .bind(format!("catalog-constraint-{suffix}"))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO merchant.stores (id, merchant_account_id, code, name) \
             VALUES ($1, $2, 'catalog-test', 'Catalog Test')",
        )
        .bind(store_id)
        .bind(account_id)
        .execute(&pool)
        .await
        .unwrap();
        for (product_id, handle) in [(product_a, "product-a"), (product_b, "product-b")] {
            sqlx::query(
                "INSERT INTO catalog.products \
                 (id, merchant_account_id, store_id, handle, title) \
                 VALUES ($1, $2, $3, $4, $4)",
            )
            .bind(product_id)
            .bind(account_id)
            .bind(store_id)
            .bind(handle)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO catalog.product_options \
             (id, merchant_account_id, store_id, product_id, name, position) \
             VALUES ($1, $2, $3, $4, 'Color', 0)",
        )
        .bind(option_a)
        .bind(account_id)
        .bind(store_id)
        .bind(product_a)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_option_values \
             (id, merchant_account_id, store_id, product_id, option_id, value, position) \
             VALUES ($1, $2, $3, $4, $5, 'Blue', 0)",
        )
        .bind(option_value_a)
        .bind(account_id)
        .bind(store_id)
        .bind(product_a)
        .bind(option_a)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO catalog.product_variants \
             (id, merchant_account_id, store_id, product_id, title) \
             VALUES ($1, $2, $3, $4, 'Default')",
        )
        .bind(variant_b)
        .bind(account_id)
        .bind(store_id)
        .bind(product_b)
        .execute(&pool)
        .await
        .unwrap();

        let cross_product = sqlx::query(
            "INSERT INTO catalog.variant_selected_options \
             (merchant_account_id, store_id, product_id, variant_id, \
              option_id, option_value_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(account_id)
        .bind(store_id)
        .bind(product_b)
        .bind(variant_b)
        .bind(option_a)
        .bind(option_value_a)
        .execute(&pool)
        .await;
        assert!(cross_product.is_err());

        sqlx::query("DELETE FROM merchant.stores WHERE id = $1")
            .bind(store_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM merchant.merchant_accounts WHERE id = $1")
            .bind(account_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
