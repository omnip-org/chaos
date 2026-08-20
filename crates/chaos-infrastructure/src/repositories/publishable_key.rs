use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chaos_application::{
    ApplicationError,
    ports::{
        AdminActor, GeneratedPublishableKeyMaterial, IdempotencyRequest, MachineActor,
        PublishableKeyCreationStatus, PublishableKeyListItem, PublishableKeyMaterialGenerator,
        PublishableKeyRepository,
    },
};
use chaos_domain::{
    identity::UserId,
    store::{PublishableKey, PublishableKeyId, PublishableKeyScope, SalesChannelId, StoreId},
};
use rand::Rng;
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::idempotency::{self, IdempotencyScope};

const CREATE_PUBLISHABLE_KEY_OPERATION: &str = "publishable_keys.create.v1";
const REVOKE_PUBLISHABLE_KEY_OPERATION: &str = "publishable_keys.revoke.v1";

#[derive(Default)]
pub struct SecurePublishableKeyMaterialGenerator;

impl PublishableKeyMaterialGenerator for SecurePublishableKeyMaterialGenerator {
    fn generate(&self) -> GeneratedPublishableKeyMaterial {
        let mut identifier_bytes = [0_u8; 12];
        let mut secret_bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut identifier_bytes);
        rand::rng().fill_bytes(&mut secret_bytes);
        let key_identifier = URL_SAFE_NO_PAD.encode(identifier_bytes);
        let secret = URL_SAFE_NO_PAD.encode(secret_bytes);
        let plaintext = format!("cc_v1_publishable_{key_identifier}_{secret}");
        let secret_digest = Sha256::digest(plaintext.as_bytes()).into();
        let display_suffix = secret[secret.len() - 4..].to_owned();

        GeneratedPublishableKeyMaterial {
            key_identifier,
            secret_digest,
            display_suffix,
            plaintext: SecretString::from(plaintext),
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
        material: &GeneratedPublishableKeyMaterial,
        idempotency: &IdempotencyRequest,
    ) -> Result<PublishableKeyCreationStatus, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(&mut transaction, &actor).await?;
        require_store(&mut transaction, publishable_key.store_id()).await?;
        let scope = IdempotencyScope::Store(actor.store_id().as_uuid());
        if idempotency::reserve(
            &mut transaction,
            &scope,
            CREATE_PUBLISHABLE_KEY_OPERATION,
            idempotency,
        )
        .await?
        .is_some()
        {
            transaction.commit().await.map_err(database_error)?;
            return Ok(PublishableKeyCreationStatus::Replayed);
        }

        sqlx::query(
            "INSERT INTO commerce.publishable_keys \
             (id, store_id, key_identifier, secret_digest, display_suffix, name, \
              created_by_user_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(publishable_key.id().as_uuid())
        .bind(publishable_key.store_id().as_uuid())
        .bind(&material.key_identifier)
        .bind(material.secret_digest.as_slice())
        .bind(&material.display_suffix)
        .bind(publishable_key.name())
        .bind(actor.audit_user_id().as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;

        for key_scope in publishable_key.scopes() {
            sqlx::query(
                "INSERT INTO commerce.publishable_key_scopes \
                 (publishable_key_id, scope) \
                 VALUES ($1, $2::commerce.publishable_key_scope)",
            )
            .bind(publishable_key.id().as_uuid())
            .bind(key_scope.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }

        idempotency::complete(
            &mut transaction,
            &scope,
            CREATE_PUBLISHABLE_KEY_OPERATION,
            idempotency,
            201,
            json!({ "data": { "id": publishable_key.id().as_uuid() } }),
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(PublishableKeyCreationStatus::Created)
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
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                Vec<String>,
                OffsetDateTime,
                Option<OffsetDateTime>,
            ),
        >(
            "SELECT key.id, key.name, key.key_identifier, key.display_suffix::text, \
                    ARRAY( \
                        SELECT scope.scope::text \
                        FROM commerce.publishable_key_scopes AS scope \
                        WHERE scope.publishable_key_id = key.id \
                        ORDER BY scope.scope::text \
                    ), \
                    key.created_at, key.revoked_at \
             FROM commerce.publishable_keys AS key \
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
            .map(
                |(id, name, key_identifier, display_suffix, scopes, created_at, revoked_at)| {
                    Ok(PublishableKeyListItem {
                        id: PublishableKeyId::from_uuid(id),
                        name,
                        key_identifier,
                        display_suffix,
                        scopes: scopes
                            .into_iter()
                            .map(|scope| {
                                PublishableKeyScope::parse(&scope)
                                    .ok_or_else(|| corrupt_enum("Publishable Key scope", &scope))
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                        created_at,
                        revoked_at,
                    })
                },
            )
            .collect()
    }

    async fn revoke(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        publishable_key_id: PublishableKeyId,
        idempotency: &IdempotencyRequest,
    ) -> Result<(), ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        set_context(&mut transaction, &actor).await?;
        require_store(&mut transaction, store_id).await?;
        let scope = IdempotencyScope::Store(actor.store_id().as_uuid());
        if idempotency::reserve(
            &mut transaction,
            &scope,
            REVOKE_PUBLISHABLE_KEY_OPERATION,
            idempotency,
        )
        .await?
        .is_some()
        {
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        }

        let result = sqlx::query(
            "UPDATE commerce.publishable_keys \
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

        idempotency::complete(
            &mut transaction,
            &scope,
            REVOKE_PUBLISHABLE_KEY_OPERATION,
            idempotency,
            204,
            json!({ "data": {} }),
        )
        .await?;
        transaction.commit().await.map_err(database_error)
    }

    async fn authenticate(
        &self,
        presented_key: &SecretString,
    ) -> Result<Option<MachineActor>, ApplicationError> {
        let Some(key_identifier) = parse_key_identifier(presented_key.expose_secret()) else {
            return Ok(None);
        };
        let digest: [u8; 32] = Sha256::digest(presented_key.expose_secret().as_bytes()).into();
        let row = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>, Vec<String>, Uuid)>(
            "SELECT publishable_key_id, store_id, sales_channel_id, \
                    scopes, created_by_user_id \
             FROM commerce.authenticate_publishable_key($1, $2)",
        )
        .bind(key_identifier)
        .bind(digest.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;

        row.map(
            |(publishable_key_id, store_id, sales_channel_id, scopes, created_by_user_id)| {
                Ok(MachineActor {
                    publishable_key_id: PublishableKeyId::from_uuid(publishable_key_id),
                    store_id: StoreId::from_uuid(store_id),
                    sales_channel_id: sales_channel_id.map(SalesChannelId::from_uuid),
                    scopes: scopes
                        .into_iter()
                        .map(|scope| {
                            PublishableKeyScope::parse(&scope)
                                .ok_or_else(|| corrupt_enum("Publishable Key scope", &scope))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
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

fn parse_key_identifier(presented_key: &str) -> Option<&str> {
    let remainder = presented_key.strip_prefix("cc_")?;
    let (version, remainder) = remainder.split_once('_')?;
    let (class, credential) = remainder.split_once('_')?;
    if version != "v1"
        || class != "publishable"
        || !credential.is_ascii()
        || credential.len() != 60
        || credential.as_bytes()[16] != b'_'
    {
        return None;
    }
    let identifier = &credential[..16];
    let secret = &credential[17..];
    if !identifier.bytes().all(is_base64url_byte) || !secret.bytes().all(is_base64url_byte) {
        return None;
    }
    Some(identifier)
}

fn is_base64url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn corrupt_enum(name: &str, value: &str) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains unknown {name}: {value}"))
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
    use chaos_domain::{identity::UserId, store::PublishableKeyScope};
    use sqlx::postgres::PgPoolOptions;

    use crate::repositories::PostgresStoreReadRepository;

    use super::*;

    #[test]
    fn generated_key_has_parseable_versioned_shape() {
        let generated = SecurePublishableKeyMaterialGenerator.generate();
        let plaintext = generated.plaintext.expose_secret();

        assert_eq!(
            parse_key_identifier(plaintext),
            Some(generated.key_identifier.as_str())
        );
        let expected_digest: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();
        assert_eq!(generated.secret_digest, expected_digest);
        assert!(plaintext.starts_with("cc_v1_publishable_"));
        assert!(plaintext.ends_with(&generated.display_suffix));
    }

    #[test]
    fn malformed_key_is_rejected_before_database_lookup() {
        assert_eq!(parse_key_identifier("cc_v1_publishable_short_secret"), None);
        assert_eq!(parse_key_identifier("not-a-key"), None);
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
            Arc::new(SecurePublishableKeyMaterialGenerator),
        );
        let authentication = PublishableKeyAuthentication::new(repository);
        let creation_input = || CreatePublishableKeyInput {
            actor: actor.clone(),
            store_id,
            name: "Storefront production".into(),
            scopes: vec!["catalog:read".into(), "orders:read".into()],
            idempotency: IdempotencyRequest {
                key: format!("create-{unique_suffix}"),
                request_fingerprint: [41; 32],
            },
        };

        let issued = management.create(creation_input()).await.unwrap();
        let publishable_key_id = issued.publishable_key.id();
        let plaintext = issued.plaintext;
        let stored_digest: Vec<u8> =
            sqlx::query_scalar("SELECT secret_digest FROM commerce.publishable_keys WHERE id = $1")
                .bind(publishable_key_id.as_uuid())
                .fetch_one(&owner_pool)
                .await
                .unwrap();
        let expected_digest: [u8; 32] = Sha256::digest(plaintext.expose_secret().as_bytes()).into();
        assert_eq!(stored_digest, expected_digest);

        let page = management
            .list(actor.clone(), store_id, None, 20)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, publishable_key_id);
        assert_eq!(page.items[0].revoked_at, None);

        let machine_actor = authentication
            .authenticate(
                &plaintext,
                &[
                    PublishableKeyScope::CatalogRead,
                    PublishableKeyScope::OrdersRead,
                ],
            )
            .await
            .unwrap();
        assert_eq!(machine_actor.store_id, store_id);
        assert!(
            authentication
                .authenticate(
                    &plaintext,
                    &[
                        PublishableKeyScope::CatalogRead,
                        PublishableKeyScope::CartsWrite
                    ],
                )
                .await
                .is_err()
        );

        let replay = management.create(creation_input()).await;
        assert!(matches!(
            replay,
            Err(ApplicationError::Conflict {
                code: "publishable_key_secret_already_issued",
                ..
            })
        ));

        let revoke_request = || IdempotencyRequest {
            key: format!("revoke-{unique_suffix}"),
            request_fingerprint: [42; 32],
        };
        management
            .revoke(
                actor.clone(),
                store_id,
                publishable_key_id,
                revoke_request(),
            )
            .await
            .unwrap();
        management
            .revoke(
                actor.clone(),
                store_id,
                publishable_key_id,
                revoke_request(),
            )
            .await
            .unwrap();
        assert!(matches!(
            authentication
                .authenticate(&plaintext, &[PublishableKeyScope::CatalogRead])
                .await,
            Err(ApplicationError::Unauthorized)
        ));
        let page = management.list(actor, store_id, None, 20).await.unwrap();
        assert!(page.items[0].revoked_at.is_some());

        sqlx::query(
            "DELETE FROM integration.idempotency_records \
             WHERE scope = 'store' AND scope_id = $1",
        )
        .bind(store_id.as_uuid())
        .execute(&owner_pool)
        .await
        .unwrap();
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
