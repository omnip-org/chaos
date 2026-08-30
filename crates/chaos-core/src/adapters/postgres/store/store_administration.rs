use crate::{
    ApplicationError,
    contracts::{AdminActor, SalesChannelAdminItem, ShippingCountryAdminItem, StoreAdminItem},
    error::database_error,
};
use chaos_domain::{
    CurrencyCode, RegionCode,
    store::{
        SalesChannel, SalesChannelId, SalesChannelStatus, Store, StoreId, StoreStatus,
        StorefrontOrigin,
    },
};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresStoreAdministrationRepository {
    pool: PgPool,
}

impl PostgresStoreAdministrationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
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

type StoreRow = (
    Uuid,
    String,
    String,
    String,
    Option<serde_json::Value>,
    String,
    OffsetDateTime,
    OffsetDateTime,
);

type ChannelRow = (Uuid, String, String, String, OffsetDateTime, OffsetDateTime);

type ShippingCountryRow = (String, bool, OffsetDateTime, OffsetDateTime);

impl PostgresStoreAdministrationRepository {
    pub(crate) async fn get_store(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Option<StoreAdminItem>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let row = sqlx::query_as::<_, StoreRow>(
            "SELECT id, name, region::text, currency::text, meta, \
                    status::text, created_at, updated_at \
             FROM commerce.stores WHERE id = $1",
        )
        .bind(store_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        row.map(store_item).transpose()
    }

    pub(crate) async fn update_store(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        name: &str,
        region: RegionCode,
        meta: Option<serde_json::Value>,
    ) -> Result<StoreId, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let result = sqlx::query(
            "UPDATE commerce.stores SET name = $2, region = $3, \
                    meta = $4, updated_at = CURRENT_TIMESTAMP \
             WHERE id = $1",
        )
        .bind(store_id.as_uuid())
        .bind(name)
        .bind(region.as_str())
        .bind(meta)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(store_not_found(store_id));
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(store_id)
    }

    pub(crate) async fn list_shipping_countries(
        &self,
        actor: AdminActor,
        store_id: StoreId,
    ) -> Result<Option<Vec<ShippingCountryAdminItem>>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if !store_exists(&mut transaction, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, ShippingCountryRow>(
            "SELECT country_code::text, enabled, created_at, updated_at \
             FROM commerce.store_shipping_countries \
             WHERE store_id = $1 ORDER BY country_code",
        )
        .bind(store_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(shipping_country_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub(crate) async fn set_shipping_country(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        country_code: &str,
        enabled: bool,
    ) -> Result<ShippingCountryAdminItem, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM commerce.stores \
             WHERE id = $1 AND status = 'active' FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| store_not_found(store_id))?;
        let row = sqlx::query_as::<_, ShippingCountryRow>(
            "INSERT INTO commerce.store_shipping_countries \
             (store_id, country_code, enabled) VALUES ($1, $2, $3) \
             ON CONFLICT (store_id, country_code) DO UPDATE SET \
                 enabled = EXCLUDED.enabled, updated_at = CURRENT_TIMESTAMP \
             RETURNING country_code::text, enabled, created_at, updated_at",
        )
        .bind(store_id.as_uuid())
        .bind(country_code)
        .bind(enabled)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        shipping_country_item(row)
    }

    pub(crate) async fn change_store_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        status: StoreStatus,
    ) -> Result<StoreId, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM commerce.stores WHERE id = $1 FOR UPDATE")
            .bind(store_id.as_uuid())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| store_not_found(store_id))?;
        if status == StoreStatus::Active {
            let active_channel_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM commerce.channels \
                 WHERE store_id = $1 \
                   AND status = 'active')",
            )
            .bind(store_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            Store::validate_activation(active_channel_exists)?;
        }
        sqlx::query(
            "UPDATE commerce.stores SET status = $2::commerce.store_status, \
                    updated_at = CURRENT_TIMESTAMP \
             WHERE id = $1",
        )
        .bind(store_id.as_uuid())
        .bind(status.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(store_id)
    }

    pub(crate) async fn list_sales_channels(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<SalesChannelId>,
        limit: u16,
    ) -> Result<Option<Vec<SalesChannelAdminItem>>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        if !store_exists(&mut transaction, store_id).await? {
            return Ok(None);
        }
        let rows = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, origin, status::text, \
                    created_at, updated_at FROM commerce.channels \
             WHERE store_id = $1 \
               AND ($2::uuid IS NULL OR id > $2) ORDER BY id ASC LIMIT $3",
        )
        .bind(store_id.as_uuid())
        .bind(after.map(SalesChannelId::as_uuid))
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter()
            .map(channel_item)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub(crate) async fn get_sales_channel(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        channel_id: SalesChannelId,
    ) -> Result<Option<SalesChannelAdminItem>, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let row = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, origin, status::text, \
                    created_at, updated_at FROM commerce.channels \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        row.map(channel_item).transpose()
    }

    pub(crate) async fn create_sales_channel(
        &self,
        actor: AdminActor,
        channel: &SalesChannel,
    ) -> Result<SalesChannelId, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        require_writable_store(&mut transaction, channel.store_id()).await?;
        sqlx::query(
            "INSERT INTO commerce.channels \
             (id, store_id, name, origin, status) \
             VALUES ($1, $2, $3, $4, 'active')",
        )
        .bind(channel.id().as_uuid())
        .bind(channel.store_id().as_uuid())
        .bind(channel.name())
        .bind(channel.origin().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_channel_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(channel.id())
    }

    pub(crate) async fn update_sales_channel(
        &self,
        actor: AdminActor,
        channel_id: SalesChannelId,
        replacement: &SalesChannel,
    ) -> Result<SalesChannelId, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let result = sqlx::query(
            "UPDATE commerce.channels SET name = $3, origin = $4, \
                    updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(replacement.store_id().as_uuid())
        .bind(channel_id.as_uuid())
        .bind(replacement.name())
        .bind(replacement.origin().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(map_channel_error)?;
        if result.rows_affected() == 0 {
            return Err(channel_not_found(channel_id));
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(channel_id)
    }

    pub(crate) async fn change_sales_channel_status(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        channel_id: SalesChannelId,
        status: SalesChannelStatus,
    ) -> Result<SalesChannelId, ApplicationError> {
        let mut transaction = self.begin(&actor).await?;
        let current_status = sqlx::query_scalar::<_, String>(
            "SELECT status::text FROM commerce.channels \
             WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| channel_not_found(channel_id))?;
        if status == SalesChannelStatus::Archived && current_status == "active" {
            let active_channel_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM commerce.channels \
                 WHERE store_id = $1 AND status = 'active'",
            )
            .bind(store_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            if active_channel_count <= 1 {
                return Err(ApplicationError::Validation {
                    violations: vec![chaos_domain::FieldViolation {
                        field: "channels",
                        reason: "must contain at least one active channel".into(),
                    }],
                });
            }
        }
        sqlx::query(
            "UPDATE commerce.channels \
             SET status = $3::commerce.sales_channel_status, updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(status.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(channel_id)
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

async fn require_writable_store(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    let writable: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.stores \
                             WHERE id = $1 AND status = 'active')",
    )
    .bind(store_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if writable {
        Ok(())
    } else {
        Err(store_not_found(store_id))
    }
}

fn store_item(row: StoreRow) -> Result<StoreAdminItem, ApplicationError> {
    let (id, name, region, currency, meta, status, created_at, updated_at) = row;
    Ok(StoreAdminItem {
        id: StoreId::from_uuid(id),
        name,
        region: RegionCode::parse(&region)?,
        currency: CurrencyCode::parse(&currency)?,
        meta,
        status: StoreStatus::parse(&status).ok_or_else(corrupt_status)?,
        created_at,
        updated_at,
    })
}

fn channel_item(row: ChannelRow) -> Result<SalesChannelAdminItem, ApplicationError> {
    let (id, name, origin, status, created_at, updated_at) = row;
    Ok(SalesChannelAdminItem {
        id: SalesChannelId::from_uuid(id),
        name,
        origin: StorefrontOrigin::parse(origin)?,
        status: SalesChannelStatus::parse(&status).ok_or_else(corrupt_status)?,
        created_at,
        updated_at,
    })
}

fn shipping_country_item(
    row: ShippingCountryRow,
) -> Result<ShippingCountryAdminItem, ApplicationError> {
    let (country_code, enabled, created_at, updated_at) = row;
    Ok(ShippingCountryAdminItem {
        country_code,
        enabled,
        created_at,
        updated_at,
    })
}

fn corrupt_status() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains an unknown status"))
}

fn map_channel_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database_error) = &error {
        let conflict = match database_error.constraint() {
            Some("channels_origin_key") => Some(ApplicationError::Conflict {
                code: "channel_origin_taken",
                message: "the storefront origin is already in use by another channel",
            }),
            _ => None,
        };
        if let Some(conflict) = conflict {
            return conflict;
        }
    }
    database_error(error)
}

fn store_not_found(store_id: StoreId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store",
        id: store_id.as_uuid().to_string(),
    }
}

fn channel_not_found(channel_id: SalesChannelId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "channel",
        id: channel_id.as_uuid().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        contracts::AdminActor,
        store::{
            ChangeSalesChannelStatusInput, ChangeStoreStatusInput, CreateSalesChannelInput,
            SetShippingCountryInput, StoreAdministration, StoreQueries, UpdateSalesChannelInput,
            UpdateStoreInput,
        },
    };
    use chaos_domain::identity::UserId;
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn administers_store_lifecycle_and_sales_channels_with_isolation() {
        let database_url =
            std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
        let owner_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let runtime_pool = PgPoolOptions::new()
            .max_connections(3)
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
        let initial_channel_id = SalesChannelId::new();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(owner_id.as_uuid())
            .bind(format!("store-admin-owner-{suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        for id in [store_id, other_store_id] {
            sqlx::query(
                "INSERT INTO commerce.stores (id, name) \
                 VALUES ($1, 'Admin Store')",
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
            "INSERT INTO commerce.store_shipping_countries (store_id, country_code) \
             VALUES ($1, 'US')",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.channels \
             (id, store_id, name, origin) \
             VALUES ($1, $2, 'Online Store', $3)",
        )
        .bind(initial_channel_id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(format!("https://{suffix}.default.example.test"))
        .execute(&owner_pool)
        .await
        .unwrap();

        let queries = StoreQueries::new(Arc::new(
            crate::adapters::postgres::PostgresStoreReadRepository::new(runtime_pool.clone()),
        ));
        let owner = queries.authorize(owner_id, store_id).await.unwrap();
        let service = StoreAdministration::new(Arc::new(
            PostgresStoreAdministrationRepository::new(runtime_pool),
        ));

        assert_eq!(
            service
                .get_store(AdminActor::Store(owner), store_id)
                .await
                .unwrap()
                .status,
            StoreStatus::Active
        );
        let shipping_countries = service
            .list_shipping_countries(AdminActor::Store(owner), store_id)
            .await
            .unwrap();
        assert_eq!(shipping_countries.len(), 1);
        assert_eq!(shipping_countries[0].country_code, "US");
        assert!(shipping_countries[0].enabled);
        let canada = service
            .set_shipping_country(SetShippingCountryInput {
                actor: AdminActor::Store(owner),
                store_id,
                country_code: " ca ".into(),
                enabled: true,
            })
            .await
            .unwrap();
        assert_eq!(canada.country_code, "CA");
        assert!(canada.enabled);
        let shipping_countries = service
            .list_shipping_countries(AdminActor::Store(owner), store_id)
            .await
            .unwrap();
        assert_eq!(
            shipping_countries
                .iter()
                .map(|item| item.country_code.as_str())
                .collect::<Vec<_>>(),
            ["CA", "US"]
        );
        service
            .update_store(UpdateStoreInput {
                actor: AdminActor::Store(owner),
                store_id,
                name: "Updated Admin Store".into(),
                region: "SG".into(),
                meta: None,
            })
            .await
            .unwrap();
        let updated = service
            .get_store(AdminActor::Store(owner), store_id)
            .await
            .unwrap();
        assert_eq!(updated.currency.as_str(), "USD");
        service
            .activate_store(ChangeStoreStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .get_store(AdminActor::Store(owner), store_id)
                .await
                .unwrap()
                .status,
            StoreStatus::Active
        );

        let channel_id = service
            .create_sales_channel(CreateSalesChannelInput {
                actor: AdminActor::Store(owner),
                store_id,
                name: "Mobile App".into(),
                origin: format!("https://{suffix}.mobile.example.test"),
            })
            .await
            .unwrap();
        let page = service
            .list_sales_channels(AdminActor::Store(owner), store_id, None, 20)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 2);
        service
            .update_sales_channel(UpdateSalesChannelInput {
                actor: AdminActor::Store(owner),
                store_id,
                channel_id,
                name: "Updated Mobile App".into(),
                origin: format!("https://{suffix}.updated.example.test"),
            })
            .await
            .unwrap();
        service
            .archive_sales_channel(ChangeSalesChannelStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
                channel_id,
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .get_sales_channel(AdminActor::Store(owner), store_id, channel_id)
                .await
                .unwrap()
                .status,
            SalesChannelStatus::Archived
        );
        service
            .activate_sales_channel(ChangeSalesChannelStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
                channel_id,
            })
            .await
            .unwrap();
        service
            .archive_sales_channel(ChangeSalesChannelStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
                channel_id: initial_channel_id,
            })
            .await
            .unwrap();
        let last_archive = service
            .archive_sales_channel(ChangeSalesChannelStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
                channel_id,
            })
            .await;
        assert!(matches!(
            last_archive,
            Err(ApplicationError::Validation { .. })
        ));
        assert!(
            service
                .get_sales_channel(AdminActor::Store(owner), other_store_id, channel_id)
                .await
                .is_err()
        );
        service
            .archive_store(ChangeStoreStatusInput {
                actor: AdminActor::Store(owner),
                store_id,
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .get_store(AdminActor::Store(owner), store_id)
                .await
                .unwrap()
                .status,
            StoreStatus::Inactive
        );
    }
}
