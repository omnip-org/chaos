use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, GeneratedPublishableKey, MachineActor, PublishableKeyGenerator,
        PublishableKeyListItem, PublishableKeyRepository,
    },
};
use chaos_domain::{
    identity::UserId,
    store::{PublishableKey, PublishableKeyId, SalesChannelId, StoreId},
};
use rand::Rng;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

const PUBLIC_KEY_PREFIX: &str = "pk_";
const PUBLIC_KEY_LENGTH: usize = 24;
const PUBLIC_KEY_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz123456789";

#[derive(Default)]
pub struct DefaultPublishableKeyGenerator;

impl PublishableKeyGenerator for DefaultPublishableKeyGenerator {
    fn generate(&self) -> GeneratedPublishableKey {
        let mut random = [0_u8; PUBLIC_KEY_LENGTH];
        rand::rng().fill_bytes(&mut random);
        let suffix = random
            .into_iter()
            .map(|byte| PUBLIC_KEY_ALPHABET[usize::from(byte) % PUBLIC_KEY_ALPHABET.len()] as char)
            .collect::<String>();
        GeneratedPublishableKey {
            public_key: format!("{PUBLIC_KEY_PREFIX}{suffix}"),
        }
    }
}

#[derive(Clone)]
pub struct PostgresPublishableKeyRepository {
    pool: PgPool,
}

impl PostgresPublishableKeyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PublishableKeyRepository for PostgresPublishableKeyRepository {
    async fn create(
        &self,
        actor: AdminActor,
        publishable_key: &PublishableKey,
        generated_key: &GeneratedPublishableKey,
    ) -> Result<(PublishableKeyId, String), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(&mut transaction, &actor).await?;
        require_store(&mut transaction, publishable_key.store_id()).await?;
        sqlx::query(
            "INSERT INTO commerce.store_publishable_keys \
             (id, store_id, public_key, name, created_by_user_id) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(publishable_key.id().as_uuid())
        .bind(publishable_key.store_id().as_uuid())
        .bind(&generated_key.public_key)
        .bind(publishable_key.name())
        .bind(actor.audit_user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;

        transaction.commit().await.map_err(database_error)?;
        Ok((publishable_key.id(), generated_key.public_key.clone()))
    }

    async fn list(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        after: Option<PublishableKeyId>,
        limit: u16,
    ) -> Result<Vec<PublishableKeyListItem>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(&mut transaction, &actor).await?;
        require_store(&mut transaction, store_id).await?;
        let rows =
            sqlx::query_as::<_, (Uuid, String, String, OffsetDateTime, Option<OffsetDateTime>)>(
                "SELECT key.id, key.name, key.public_key, key.created_at, key.revoked_at \
             FROM commerce.store_publishable_keys AS key \
             WHERE key.store_id = $1 \
               AND ($2::uuid IS NULL OR key.id > $2) \
             ORDER BY key.id ASC \
             LIMIT $3",
            )
            .bind(store_id.as_uuid())
            .bind(after.map(PublishableKeyId::as_uuid))
            .bind(i64::from(limit))
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;

        rows.into_iter()
            .map(|(id, name, public_key, created_at, revoked_at)| {
                Ok(PublishableKeyListItem {
                    id: PublishableKeyId::from_uuid(id),
                    name,
                    public_key,
                    created_at,
                    revoked_at,
                })
            })
            .collect()
    }

    async fn revoke(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        publishable_key_id: PublishableKeyId,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(&mut transaction, &actor).await?;
        require_store(&mut transaction, store_id).await?;
        let result = sqlx::query(
            "UPDATE commerce.store_publishable_keys \
             SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP), \
                 revoked_by_user_id = COALESCE(revoked_by_user_id, $3), \
                 updated_at = CURRENT_TIMESTAMP \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(store_id.as_uuid())
        .bind(publishable_key_id.as_uuid())
        .bind(actor.audit_user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(ApplicationError::NotFound {
                resource: "Publishable Key",
                id: publishable_key_id.as_uuid().to_string(),
            });
        }

        transaction.commit().await.map_err(database_error)
    }

    async fn authenticate(
        &self,
        presented_key: &str,
    ) -> Result<Option<MachineActor>, ApplicationError> {
        if !valid_public_key(presented_key) {
            return Ok(None);
        }
        let row = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>, Uuid)>(
            "SELECT publishable_key_id, store_id, sales_channel_id, created_by_user_id \
             FROM commerce.authenticate_publishable_key($1)",
        )
        .bind(presented_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;

        row.map(
            |(publishable_key_id, store_id, sales_channel_id, created_by_user_id)| {
                Ok(MachineActor {
                    publishable_key_id: PublishableKeyId::from_uuid(publishable_key_id),
                    store_id: StoreId::from_uuid(store_id),
                    sales_channel_id: sales_channel_id.map(SalesChannelId::from_uuid),
                    created_by_user_id: UserId::from_uuid(created_by_user_id),
                })
            },
        )
        .transpose()
    }
}

async fn set_context(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &AdminActor,
) -> Result<(), ApplicationError> {
    match actor {
        AdminActor::Store(_) => {
            sqlx::query("SELECT set_config('app.user_id', $1, true)")
                .bind(actor.audit_user_id().as_uuid().to_string())
                .execute(&mut **transaction)
                .await
                .map_err(database_error)?;
        }
        AdminActor::Machine(_) => {
            sqlx::query("SELECT set_config('app.user_id', '', true)")
                .execute(&mut **transaction)
                .await
                .map_err(database_error)?;
        }
    }
    sqlx::query("SELECT set_config('app.store_id', $1, true)")
        .bind(actor.store_id().as_uuid().to_string())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(())
}

async fn require_store(
    transaction: &mut Transaction<'_, Postgres>,
    store_id: StoreId,
) -> Result<(), ApplicationError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.stores WHERE id = $1 AND status = 'active')",
    )
    .bind(store_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if !exists {
        return Err(ApplicationError::Forbidden);
    }
    Ok(())
}

fn valid_public_key(value: &str) -> bool {
    value.len() == PUBLIC_KEY_PREFIX.len() + PUBLIC_KEY_LENGTH
        && value.starts_with(PUBLIC_KEY_PREFIX)
        && value[PUBLIC_KEY_PREFIX.len()..]
            .bytes()
            .all(|byte| PUBLIC_KEY_ALPHABET.contains(&byte))
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chaos_application::store::{
        CreatePublishableKeyInput, PublishableKeyAuthentication, PublishableKeyManagement,
        StoreQueries,
    };
    use chaos_domain::identity::UserId;
    use sqlx::postgres::PgPoolOptions;

    use crate::repositories::PostgresStoreReadRepository;

    use super::*;

    #[test]
    fn generated_key_has_parseable_public_shape() {
        let generated = DefaultPublishableKeyGenerator.generate();
        assert!(valid_public_key(&generated.public_key));
        assert!(generated.public_key.starts_with(PUBLIC_KEY_PREFIX));
        assert_eq!(
            generated.public_key.len(),
            PUBLIC_KEY_PREFIX.len() + PUBLIC_KEY_LENGTH
        );
    }

    #[test]
    fn malformed_key_is_rejected_before_database_lookup() {
        assert!(!valid_public_key("pk_short"));
        assert!(!valid_public_key("not-a-key"));
    }

    #[test]
    fn old_publishable_key_shape_is_rejected() {
        assert!(!valid_public_key("cc_v1_publishable_a_secret"));
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL with migrations applied"]
    async fn manages_and_authenticates_store_scoped_publishable_keys() {
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
        let user_id = UserId::new();
        let store_id = StoreId::new();
        let unique_suffix = Uuid::now_v7().simple().to_string()[..12].to_owned();

        sqlx::query("INSERT INTO identity.users (id, email) VALUES ($1, $2)")
            .bind(user_id.as_uuid())
            .bind(format!("api-key-{unique_suffix}@example.com"))
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO commerce.stores \
             (id, code, name, status) \
             VALUES ($1, $2, 'API Test', 'active')",
        )
        .bind(store_id.as_uuid())
        .bind(format!("api-test-{unique_suffix}"))
        .execute(&owner_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO commerce.store_memberships \
             (store_id, user_id, role) VALUES ($1, $2, 'owner')",
        )
        .bind(store_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();

        let repository = Arc::new(PostgresPublishableKeyRepository::new(runtime_pool.clone()));
        let queries = StoreQueries::new(Arc::new(PostgresStoreReadRepository::new(runtime_pool)));
        let actor = AdminActor::Store(queries.authorize(user_id, store_id).await.unwrap());
        let management = PublishableKeyManagement::new(
            repository.clone(),
            Arc::new(DefaultPublishableKeyGenerator),
        );
        let authentication = PublishableKeyAuthentication::new(repository);
        let creation_input = || CreatePublishableKeyInput {
            actor: actor.clone(),
            store_id,
            name: "Storefront production".into(),
        };

        let issued = management.create(creation_input()).await.unwrap();
        let publishable_key_id = issued.publishable_key.id();
        let public_key = issued.public_key;
        let stored_public_key: String = sqlx::query_scalar(
            "SELECT public_key FROM commerce.store_publishable_keys WHERE id = $1",
        )
        .bind(publishable_key_id.as_uuid())
        .fetch_one(&owner_pool)
        .await
        .unwrap();
        assert_eq!(stored_public_key, public_key);

        let page = management
            .list(actor.clone(), store_id, None, 20)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, publishable_key_id);
        assert_eq!(page.items[0].public_key, public_key);
        assert_eq!(page.items[0].revoked_at, None);

        let machine_actor = authentication.authenticate(&public_key).await.unwrap();
        assert_eq!(machine_actor.store_id, store_id);

        management
            .revoke(actor.clone(), store_id, publishable_key_id)
            .await
            .unwrap();
        assert!(matches!(
            authentication.authenticate(&public_key).await,
            Err(ApplicationError::Unauthorized)
        ));
        let page = management.list(actor, store_id, None, 20).await.unwrap();
        assert!(page.items[0].revoked_at.is_some());

        sqlx::query("DELETE FROM commerce.stores WHERE id = $1")
            .bind(store_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM identity.users WHERE id = $1")
            .bind(user_id.as_uuid())
            .execute(&owner_pool)
            .await
            .unwrap();
    }
}
