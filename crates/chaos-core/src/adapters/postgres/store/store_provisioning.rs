use crate::{ApplicationError, error::database_error};
use chaos_domain::{
    identity::UserId,
    store::{SalesChannel, Store, StoreMembership},
};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresStoreProvisioningRepository {
    pool: PgPool,
}

impl PostgresStoreProvisioningRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

pub(crate) struct PostgresStoreProvisioningTransaction {
    transaction: Transaction<'static, Postgres>,
}

impl PostgresStoreProvisioningRepository {
    pub(crate) async fn begin(
        &self,
        user_id: UserId,
    ) -> Result<PostgresStoreProvisioningTransaction, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_user_context(&mut transaction, user_id)
            .await
            .map_err(database_error)?;
        Ok(PostgresStoreProvisioningTransaction { transaction })
    }
}

impl PostgresStoreProvisioningTransaction {
    pub(crate) async fn insert_store(&mut self, store: &Store) -> Result<(), ApplicationError> {
        crate::adapters::postgres::database::set_store_context(&mut self.transaction, store.id())
            .await
            .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO commerce.stores \
             (id, name, region, currency, meta, status) \
             VALUES ($1, $2, $3, $4, $5, 'active')",
        )
        .bind(store.id().as_uuid())
        .bind(store.name())
        .bind(store.region().as_str())
        .bind(store.currency().as_str())
        .bind(store.meta().cloned())
        .execute(&mut *self.transaction)
        .await
        .map_err(map_store_write_error)?;
        // A new Store can otherwise ship nowhere: seed its home region as the
        // first enabled shipping destination. Owners add more from there.
        sqlx::query(
            "INSERT INTO commerce.store_shipping_countries (store_id, country_code) \
             VALUES ($1, $2)",
        )
        .bind(store.id().as_uuid())
        .bind(store.region().as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(database_error)?;
        // Every Store can mark Orders shipped/delivered from day one, without
        // any carrier integration or credential setup.
        sqlx::query(
            "INSERT INTO integration.provider_accounts \
             (id, store_id, capability, provider, display_name) \
             VALUES ($1, $2, 'shipping', 'manual', 'Manual fulfillment')",
        )
        .bind(Uuid::now_v7())
        .bind(store.id().as_uuid())
        .execute(&mut *self.transaction)
        .await
        .map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn insert_owner_membership(
        &mut self,
        membership: &StoreMembership,
    ) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO commerce.store_memberships (store_id, user_id, role) \
             VALUES ($1, $2, $3::commerce.store_role)",
        )
        .bind(membership.store_id().as_uuid())
        .bind(membership.user_id().as_uuid())
        .bind(membership.role().as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn insert_initial_channel(
        &mut self,
        channel: &SalesChannel,
    ) -> Result<(), ApplicationError> {
        sqlx::query(
            "INSERT INTO commerce.channels \
             (id, store_id, name, origin, status) \
             VALUES ($1, $2, $3, $4, $5::commerce.sales_channel_status)",
        )
        .bind(channel.id().as_uuid())
        .bind(channel.store_id().as_uuid())
        .bind(channel.name())
        .bind(channel.origin().as_str())
        .bind(channel.status().as_str())
        .execute(&mut *self.transaction)
        .await
        .map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn commit(self) -> Result<(), ApplicationError> {
        self.transaction.commit().await.map_err(database_error)
    }
}

fn map_store_write_error(error: sqlx::Error) -> ApplicationError {
    database_error(error)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::store::{CreateStore, CreateStoreInput};
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn provisions_a_store_with_owner_membership_currency_and_constraints() {
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
        let unique_suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_user_id.as_uuid())
            .bind(format!("store-owner-{unique_suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();

        let service = CreateStore::new(Arc::new(PostgresStoreProvisioningRepository::new(
            runtime_pool,
        )));
        let make_input = || CreateStoreInput {
            user_id: owner_user_id,
            name: "Primary Store".into(),
            region: None,
            currency: None,
            meta: None,
            origin: format!("https://{unique_suffix}.shop.example.test"),
        };

        let output = service.execute(make_input()).await.unwrap();

        let stored: (String, String, String, String) = sqlx::query_as(
            "SELECT store.status::text, store.region::text, \
                    store.currency::text, \
                    channel.origin \
             FROM commerce.stores AS store \
             INNER JOIN commerce.channels AS channel \
                 ON channel.store_id = store.id \
             WHERE store.id = $1",
        )
        .bind(output.store_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(
            stored,
            (
                "active".into(),
                "US".into(),
                "USD".into(),
                format!("https://{unique_suffix}.shop.example.test/"),
            )
        );

        let membership_role: String = sqlx::query_scalar(
            "SELECT role::text FROM commerce.store_memberships \
             WHERE store_id = $1 AND user_id = $2",
        )
        .bind(output.store_id.as_uuid())
        .bind(owner_user_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(membership_role, "owner");

        sqlx::query("DELETE FROM commerce.stores WHERE id = $1")
            .bind(output.store_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(owner_user_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
