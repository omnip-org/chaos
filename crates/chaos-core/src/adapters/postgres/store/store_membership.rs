use crate::{
    ApplicationError,
    contracts::{StoreMembershipItem, StoreMembershipRepository},
    error::database_error,
    store::StoreActor,
};
use async_trait::async_trait;
use chaos_domain::{
    identity::UserId,
    store::{StoreId, StoreRole},
};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresStoreMembershipRepository {
    pool: PgPool,
}

impl PostgresStoreMembershipRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        actor: StoreActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        crate::adapters::postgres::database::set_admin_context(
            &mut transaction,
            Some(actor.user_id()),
            actor.store_id(),
        )
        .await
        .map_err(database_error)?;
        Ok(transaction)
    }
}

type MembershipRow = (Uuid, String, OffsetDateTime, OffsetDateTime);

#[async_trait]
impl StoreMembershipRepository for PostgresStoreMembershipRepository {
    async fn list(
        &self,
        actor: StoreActor,
        store_id: StoreId,
    ) -> Result<Vec<StoreMembershipItem>, ApplicationError> {
        require_selected_store(actor, store_id)?;
        let mut transaction = self.begin(actor).await?;
        let rows = sqlx::query_as::<_, MembershipRow>(
            "SELECT user_id, role::text, created_at, updated_at \
             FROM commerce.store_memberships WHERE store_id = $1 \
             ORDER BY created_at, user_id",
        )
        .bind(actor.store_id().as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        rows.into_iter().map(membership_item).collect()
    }

    async fn add_member(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        user_id: UserId,
    ) -> Result<StoreMembershipItem, ApplicationError> {
        require_selected_store(actor, store_id)?;
        let mut transaction = self.begin(actor).await?;
        let row = sqlx::query_as::<_, MembershipRow>(
            "INSERT INTO commerce.store_memberships (store_id, user_id, role) \
             VALUES ($1, $2, 'member') \
             ON CONFLICT (store_id, user_id) DO UPDATE SET updated_at = \
                 commerce.store_memberships.updated_at \
             RETURNING user_id, role::text, created_at, updated_at",
        )
        .bind(actor.store_id().as_uuid())
        .bind(user_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| map_membership_error(error, user_id))?;
        let item = membership_item(row)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(item)
    }

    async fn set_role(
        &self,
        actor: StoreActor,
        store_id: StoreId,
        user_id: UserId,
        role: StoreRole,
    ) -> Result<StoreMembershipItem, ApplicationError> {
        require_selected_store(actor, store_id)?;
        let mut transaction = self.begin(actor).await?;
        lock_memberships(&mut transaction, actor.store_id()).await?;
        if role == StoreRole::Member {
            protect_last_owner(&mut transaction, actor.store_id(), user_id).await?;
        }
        let row = sqlx::query_as::<_, MembershipRow>(
            "UPDATE commerce.store_memberships SET role = $3::commerce.store_role, \
                    updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND user_id = $2 \
             RETURNING user_id, role::text, created_at, updated_at",
        )
        .bind(actor.store_id().as_uuid())
        .bind(user_id.as_uuid())
        .bind(role.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| member_not_found(user_id))?;
        let item = membership_item(row)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(item)
    }

    async fn leave(&self, actor: StoreActor, store_id: StoreId) -> Result<(), ApplicationError> {
        require_selected_store(actor, store_id)?;
        let mut transaction = self.begin(actor).await?;
        lock_memberships(&mut transaction, actor.store_id()).await?;
        if actor.role() == StoreRole::Owner {
            protect_last_owner(&mut transaction, actor.store_id(), actor.user_id()).await?;
        }
        sqlx::query("DELETE FROM commerce.store_memberships WHERE store_id = $1 AND user_id = $2")
            .bind(actor.store_id().as_uuid())
            .bind(actor.user_id().as_uuid())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }
}

async fn lock_memberships(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "SELECT user_id FROM commerce.store_memberships \
         WHERE store_id = $1 ORDER BY user_id FOR UPDATE",
    )
    .bind(store_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn protect_last_owner(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    user_id: UserId,
) -> Result<(), ApplicationError> {
    let target_is_owner: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.store_memberships \
         WHERE store_id = $1 AND user_id = $2 AND role = 'owner')",
    )
    .bind(store_id.as_uuid())
    .bind(user_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if target_is_owner {
        let owner_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commerce.store_memberships \
             WHERE store_id = $1 AND role = 'owner'",
        )
        .bind(store_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?;
        if owner_count <= 1 {
            return Err(ApplicationError::Conflict {
                code: "last_store_owner",
                message: "a Store must retain at least one owner",
            });
        }
    }
    Ok(())
}

fn membership_item(row: MembershipRow) -> Result<StoreMembershipItem, ApplicationError> {
    Ok(StoreMembershipItem {
        user_id: UserId::from_uuid(row.0),
        role: StoreRole::parse(&row.1).ok_or_else(|| {
            ApplicationError::Unexpected(anyhow::anyhow!("invalid Store role in database"))
        })?,
        created_at: row.2,
        updated_at: row.3,
    })
}

fn member_not_found(user_id: UserId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "store_member",
        id: user_id.as_uuid().to_string(),
    }
}

fn require_selected_store(actor: StoreActor, store_id: StoreId) -> Result<(), ApplicationError> {
    if actor.store_id() == store_id {
        Ok(())
    } else {
        Err(ApplicationError::Forbidden)
    }
}

fn map_membership_error(error: sqlx::Error, user_id: UserId) -> ApplicationError {
    if error
        .as_database_error()
        .and_then(|error| error.constraint())
        == Some("store_memberships_user_id_fkey")
    {
        ApplicationError::NotFound {
            resource: "user",
            id: user_id.as_uuid().to_string(),
        }
    } else {
        database_error(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::store::{StoreMembershipManagement, StoreQueries};
    use sqlx::postgres::PgPoolOptions;

    use crate::adapters::postgres::PostgresStoreReadRepository;

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn manages_memberships_without_allowing_a_store_to_lose_its_last_owner() {
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
        let member_id = UserId::new();
        let store_id = StoreId::new();
        let suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        for (user_id, label) in [(owner_id, "owner"), (member_id, "member")] {
            sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
                .bind(user_id.as_uuid())
                .bind(format!("membership-{label}-{suffix}@example.com"))
                .execute(&owner_pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO commerce.stores (id, name, status) \
             VALUES ($1, 'Membership Store', 'active')",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.store_memberships (store_id, user_id, role) \
             VALUES ($1, $2, 'owner')",
        )
        .bind(store_id.as_uuid())
        .bind(owner_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let directory = StoreQueries::new(Arc::new(PostgresStoreReadRepository::new(
            runtime_pool.clone(),
        )));
        let service = StoreMembershipManagement::new(Arc::new(
            PostgresStoreMembershipRepository::new(runtime_pool),
        ));
        let owner = directory.authorize(owner_id, store_id).await.unwrap();

        assert!(matches!(
            service.leave(owner, store_id).await,
            Err(ApplicationError::Conflict {
                code: "last_store_owner",
                ..
            })
        ));

        let added = service
            .add_member(owner, store_id, member_id)
            .await
            .unwrap();
        assert_eq!(added.role, StoreRole::Member);

        let promoted = service
            .set_role(owner, store_id, member_id, StoreRole::Owner)
            .await
            .unwrap();
        assert_eq!(promoted.role, StoreRole::Owner);
        service.leave(owner, store_id).await.unwrap();
        assert!(matches!(
            directory.authorize(owner_id, store_id).await,
            Err(ApplicationError::Forbidden)
        ));

        let new_owner = directory.authorize(member_id, store_id).await.unwrap();
        assert!(matches!(
            service
                .set_role(new_owner, store_id, member_id, StoreRole::Member,)
                .await,
            Err(ApplicationError::Conflict {
                code: "last_store_owner",
                ..
            })
        ));

        sqlx::query("DELETE FROM commerce.stores WHERE id = $1")
            .bind(store_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = ANY($1)")
            .bind(vec![owner_id.as_uuid(), member_id.as_uuid()])
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
