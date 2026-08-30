#[cfg(test)]
mod tests {
    use chaos_domain::store::StoreId;
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::super::set_store_context;

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

        for (store_id, name) in [(store_a, "store-a"), (store_b, "store-b")] {
            sqlx::query(
                "INSERT INTO commerce.stores (id, name, currency, status) \
                 VALUES ($1, $2, 'USD', 'active')",
            )
            .bind(store_id)
            .bind(name)
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
        }
        for (channel_id, store_id, name) in
            [(channel_a, store_a, "web-a"), (channel_b, store_b, "web-b")]
        {
            sqlx::query(
                "INSERT INTO commerce.channels \
                 (id, store_id, name, origin) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(channel_id)
            .bind(store_id)
            .bind(name)
            .bind(format!("https://{}.example.test/", store_id.simple()))
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
        for (key_id, store_id, channel_id, public_key) in [
            (key_a, store_a, channel_a, "pk_123456789ABCDEFGHJKLMNPQ"),
            (key_b, store_b, channel_b, "pk_23456789ABCDEFGHJKLMNPQR"),
        ] {
            sqlx::query(
                "INSERT INTO commerce.channel_publishable_keys \
                 (id, store_id, channel_id, public_key, name) \
                 VALUES ($1, $2, $3, $4, 'RLS test key')",
            )
            .bind(key_id)
            .bind(store_id)
            .bind(channel_id)
            .bind(public_key)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (shopper_id, store_id) in [(shopper_a, store_a), (shopper_b, store_b)] {
            sqlx::query("INSERT INTO commerce.shoppers (id, store_id) VALUES ($1, $2)")
                .bind(shopper_id)
                .bind(store_id)
                .execute(&pool)
                .await
                .unwrap();
        }
        for (id, store_id) in [(provider_account_a, store_a), (provider_account_b, store_b)] {
            sqlx::query(
                "INSERT INTO integration.provider_accounts \
                 (id, store_id, capability, provider) \
                 VALUES ($1, $2, 'payment', 'stripe')",
            )
            .bind(id)
            .bind(store_id)
            .execute(&pool)
            .await
            .unwrap();
        }

        let mut transaction = pool.begin().await.unwrap();
        set_store_context(&mut transaction, StoreId::from_uuid(store_a))
            .await
            .unwrap();
        sqlx::query("SET LOCAL ROLE chaos_runtime")
            .execute(&mut *transaction)
            .await
            .unwrap();
        let visible_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.stores ORDER BY id")
                .fetch_all(&mut *transaction)
                .await
                .unwrap();
        let visible_key_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.channel_publishable_keys ORDER BY id")
                .fetch_all(&mut *transaction)
                .await
                .unwrap();
        let visible_channel_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.channels ORDER BY id")
                .fetch_all(&mut *transaction)
                .await
                .unwrap();
        let visible_product_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.products ORDER BY id")
                .fetch_all(&mut *transaction)
                .await
                .unwrap();
        let visible_shopper_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM commerce.shoppers ORDER BY id")
                .fetch_all(&mut *transaction)
                .await
                .unwrap();
        let visible_provider_account_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM integration.provider_accounts WHERE capability = 'payment' ORDER BY id",
        )
        .fetch_all(&mut *transaction)
        .await
        .unwrap();
        transaction.rollback().await.unwrap();

        assert_eq!(visible_ids, vec![store_a]);
        assert_eq!(visible_key_ids, vec![key_a]);
        assert_eq!(visible_channel_ids, vec![channel_a]);
        assert_eq!(visible_product_ids, vec![product_a]);
        assert_eq!(visible_shopper_ids, vec![shopper_a]);
        assert_eq!(visible_provider_account_ids, vec![provider_account_a]);

        sqlx::query("DELETE FROM integration.provider_accounts WHERE store_id = ANY($1)")
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
        sqlx::query(
            "INSERT INTO commerce.stores (id, name) \
             VALUES ($1, 'Catalog Test')",
        )
        .bind(store_id)
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
