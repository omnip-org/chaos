use crate::{ApplicationError, error::database_error};
use sqlx::{PgPool, Postgres, Transaction};

const CLEANUP_BATCH_SIZE: i32 = 500;

/// Bounded retention work that must run with the same least-privilege pools as
/// the data it removes. Identity cleanup uses `chaos_identity`; the tracking
/// token routine is a narrowly granted SECURITY DEFINER function because the
/// runtime role must not receive cross-Store delete access.
#[derive(Clone)]
pub struct PostgresMaintenance {
    runtime_pool: PgPool,
    identity_pool: PgPool,
}

impl PostgresMaintenance {
    pub fn new(runtime_pool: PgPool, identity_pool: PgPool) -> Self {
        Self {
            runtime_pool,
            identity_pool,
        }
    }

    pub async fn cleanup_expired(&self) -> Result<usize, ApplicationError> {
        let mut identity_transaction = self.identity_pool.begin().await.map_err(database_error)?;
        let mut deleted = 0_usize;
        deleted += delete_expired_authorization_requests(&mut identity_transaction).await?;
        deleted += delete_expired_authorization_codes(&mut identity_transaction).await?;
        deleted += delete_expired_access_tokens(&mut identity_transaction).await?;
        deleted += delete_expired_refresh_tokens(&mut identity_transaction).await?;
        identity_transaction
            .commit()
            .await
            .map_err(database_error)?;

        let tracking_deleted: i32 =
            sqlx::query_scalar("SELECT commerce.cleanup_expired_order_tracking_tokens($1)")
                .bind(CLEANUP_BATCH_SIZE)
                .fetch_one(&self.runtime_pool)
                .await
                .map_err(database_error)?;
        deleted += usize::try_from(tracking_deleted).unwrap_or_default();

        let integration_deleted: i32 =
            sqlx::query_scalar("SELECT integration.cleanup_terminal_rows($1)")
                .bind(CLEANUP_BATCH_SIZE)
                .fetch_one(&self.runtime_pool)
                .await
                .map_err(database_error)?;
        deleted += usize::try_from(integration_deleted).unwrap_or_default();
        Ok(deleted)
    }
}

async fn delete_expired_authorization_requests(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<usize, ApplicationError> {
    delete_rows(
        transaction,
        "DELETE FROM identity.oauth_authorization_requests AS request
          WHERE request.id IN (
              SELECT candidate.id
              FROM identity.oauth_authorization_requests AS candidate
              WHERE candidate.expires_at < CURRENT_TIMESTAMP - INTERVAL '1 hour'
                 OR (candidate.used_at IS NOT NULL
                     AND candidate.used_at < CURRENT_TIMESTAMP - INTERVAL '1 day')
              ORDER BY candidate.created_at, candidate.id
              LIMIT $1
          )
          RETURNING 1",
    )
    .await
}

async fn delete_expired_authorization_codes(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<usize, ApplicationError> {
    delete_rows(
        transaction,
        "DELETE FROM identity.oauth_authorization_codes AS code
          WHERE code.code_digest IN (
              SELECT candidate.code_digest
              FROM identity.oauth_authorization_codes AS candidate
              WHERE candidate.expires_at < CURRENT_TIMESTAMP - INTERVAL '1 hour'
                 OR (candidate.consumed_at IS NOT NULL
                     AND candidate.consumed_at < CURRENT_TIMESTAMP - INTERVAL '1 day')
              ORDER BY candidate.created_at, candidate.code_digest
              LIMIT $1
          )
          RETURNING 1",
    )
    .await
}

async fn delete_expired_access_tokens(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<usize, ApplicationError> {
    delete_rows(
        transaction,
        "DELETE FROM identity.oauth_access_tokens AS token
          WHERE token.token_digest IN (
              SELECT candidate.token_digest
              FROM identity.oauth_access_tokens AS candidate
              WHERE candidate.expires_at < CURRENT_TIMESTAMP - INTERVAL '1 day'
                 OR (candidate.revoked_at IS NOT NULL
                     AND candidate.revoked_at < CURRENT_TIMESTAMP - INTERVAL '1 day')
              ORDER BY candidate.created_at, candidate.token_digest
              LIMIT $1
          )
          RETURNING 1",
    )
    .await
}

async fn delete_expired_refresh_tokens(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<usize, ApplicationError> {
    delete_rows(
        transaction,
        "DELETE FROM identity.oauth_refresh_tokens AS token
          WHERE token.token_digest IN (
              SELECT candidate.token_digest
              FROM identity.oauth_refresh_tokens AS candidate
              WHERE candidate.expires_at < CURRENT_TIMESTAMP - INTERVAL '1 day'
                 OR (candidate.revoked_at IS NOT NULL
                     AND candidate.revoked_at < CURRENT_TIMESTAMP - INTERVAL '1 day')
              ORDER BY candidate.created_at, candidate.token_digest
              LIMIT $1
          )
          RETURNING 1",
    )
    .await
}

async fn delete_rows(
    transaction: &mut Transaction<'_, Postgres>,
    statement: &'static str,
) -> Result<usize, ApplicationError> {
    let deleted: Vec<i32> = sqlx::query_scalar(statement)
        .bind(CLEANUP_BATCH_SIZE)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(deleted.len())
}
