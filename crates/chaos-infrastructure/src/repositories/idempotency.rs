use chaos_application::{ApplicationError, ports::IdempotencyRequest};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(super) enum IdempotencyScope {
    User(Uuid),
    MerchantAccount(Uuid),
    Shopper(Uuid),
}

impl IdempotencyScope {
    fn kind(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::MerchantAccount(_) => "merchant_account",
            Self::Shopper(_) => "shopper",
        }
    }

    fn id(&self) -> Uuid {
        match self {
            Self::User(id) | Self::MerchantAccount(id) | Self::Shopper(id) => *id,
        }
    }
}

pub(super) async fn reserve(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Value>, ApplicationError> {
    let inserted = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO integration.idempotency_records \
         (id, scope, scope_id, operation, idempotency_key, request_fingerprint) \
         VALUES (uuidv7(), $1::integration.idempotency_scope, $2, $3, $4, $5) \
         ON CONFLICT (scope, scope_id, operation, idempotency_key) DO NOTHING \
         RETURNING id",
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(operation)
    .bind(&request.key)
    .bind(request.request_fingerprint.as_slice())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unexpected_database_error)?;

    if inserted.is_some() {
        return Ok(None);
    }

    let record = sqlx::query_as::<_, (Vec<u8>, Option<Value>)>(
        "SELECT request_fingerprint, response_body \
         FROM integration.idempotency_records \
         WHERE scope = $1::integration.idempotency_scope \
           AND scope_id = $2 \
           AND operation = $3 \
           AND idempotency_key = $4 \
         FOR UPDATE",
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(operation)
    .bind(&request.key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unexpected_database_error)?;

    if record.0.as_slice() != request.request_fingerprint {
        return Err(ApplicationError::Conflict {
            code: "idempotency_key_reused",
            message: "the idempotency key was already used with a different request",
        });
    }

    record.1.map(Some).ok_or_else(|| {
        ApplicationError::Unexpected(anyhow::anyhow!(
            "completed idempotency record has no response snapshot"
        ))
    })
}

pub(super) async fn complete(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
    response_status: i16,
    response_body: Value,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE integration.idempotency_records \
         SET response_status = $5, \
             response_body = $6, \
             completed_at = CURRENT_TIMESTAMP, \
             updated_at = CURRENT_TIMESTAMP \
         WHERE scope = $1::integration.idempotency_scope \
           AND scope_id = $2 \
           AND operation = $3 \
           AND idempotency_key = $4",
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(operation)
    .bind(&request.key)
    .bind(response_status)
    .bind(response_body)
    .execute(&mut **transaction)
    .await
    .map_err(unexpected_database_error)?;

    if result.rows_affected() != 1 {
        return Err(ApplicationError::Unexpected(anyhow::anyhow!(
            "idempotency record disappeared before completion"
        )));
    }
    Ok(())
}

fn unexpected_database_error(error: sqlx::Error) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}
