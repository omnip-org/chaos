// Payment configuration helpers, outbox payload parsing, provider account lookup, and shared errors.

async fn set_config(
    transaction: &mut Transaction<'static, Postgres>,
    key: &'static str,
    value: Uuid,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT set_config($1, $2, true)")
        .bind(key)
        .bind(value.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    Ok(())
}

fn format_time(value: OffsetDateTime) -> Result<String, ApplicationError> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn parse_time(value: &str) -> Result<OffsetDateTime, ApplicationError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn invalid_snapshot(error: serde_json::Error) -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "invalid payment idempotency snapshot: {error}"
    ))
}

fn outbox_aggregate_id(job: &QueueJob) -> Result<Uuid, ApplicationError> {
    job.payload
        .get("aggregate_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_provider(job: &QueueJob) -> Result<&str, ApplicationError> {
    job.payload
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_amount(job: &QueueJob) -> Result<i64, ApplicationError> {
    job.payload
        .get("amount_minor")
        .and_then(Value::as_i64)
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_currency(job: &QueueJob) -> Result<&str, ApplicationError> {
    job.payload
        .get("currency")
        .and_then(Value::as_str)
        .ok_or_else(invalid_outbox_payload)
}

fn outbox_return_url(job: &QueueJob) -> Option<String> {
    job.payload
        .get("return_url")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn invalid_outbox_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("payment outbox payload is invalid"))
}

fn provider_invalid_response() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "payment_provider",
        source: anyhow::anyhow!("provider returned an invalid reference"),
    }
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn attempt_not_found(attempt_id: PaymentAttemptId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "payment_attempt",
        id: attempt_id.as_uuid().to_string(),
    }
}

fn refund_not_found(refund_id: RefundId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "refund",
        id: refund_id.as_uuid().to_string(),
    }
}

async fn load_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    id: PaymentProviderAccountId,
) -> Result<Option<PaymentProviderAccountDetail>, ApplicationError> {
    sqlx::query_as::<_, ProviderAccountRow>(
        "SELECT id, provider, display_name, enabled, \
                credential_secret_reference IS NOT NULL AND webhook_secret_reference IS NOT NULL, \
                readiness_status, readiness_checked_at, readiness_valid_until, \
                COALESCE(readiness_snapshot->'blocker_codes', '[]'::jsonb), \
                credential_rotation_expires_at, webhook_rotation_expires_at, \
                created_at, updated_at FROM commerce.provider_accounts \
         WHERE store_id = $1 AND id = $2",
    )
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(provider_account_detail)
    .transpose()
}

fn provider_account_detail(
    row: ProviderAccountRow,
) -> Result<PaymentProviderAccountDetail, ApplicationError> {
    Ok(PaymentProviderAccountDetail {
        account: PaymentProviderAccount::rehydrate(
            PaymentProviderAccountId::from_uuid(row.0),
            row.1,
            row.2,
            row.3,
        )?,
        credentials_configured: row.4,
        readiness_status: match row.5.as_str() {
            "unchecked" => PaymentProviderReadinessStatus::Unchecked,
            "ready" => PaymentProviderReadinessStatus::Ready,
            "action_required" => PaymentProviderReadinessStatus::ActionRequired,
            _ => return Err(corrupt_state()),
        },
        readiness_checked_at: row.6,
        readiness_valid_until: row.7,
        readiness_blocker_codes: serde_json::from_value(row.8).map_err(|_| corrupt_state())?,
        credential_rotation_expires_at: row.9,
        webhook_rotation_expires_at: row.10,
        created_at: row.11,
        updated_at: row.12,
    })
}

fn readiness_status(readiness: &PaymentProviderReadiness) -> PaymentProviderReadinessStatus {
    if readiness.ready {
        PaymentProviderReadinessStatus::Ready
    } else {
        PaymentProviderReadinessStatus::ActionRequired
    }
}

async fn replay_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    snapshot: Value,
) -> Result<PaymentProviderAccountDetail, ApplicationError> {
    let id = snapshot
        .pointer("/data/id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(PaymentProviderAccountId::from_uuid)
        .ok_or_else(corrupt_state)?;
    load_provider_account(transaction, store_id, id)
        .await?
        .ok_or_else(corrupt_state)
}

async fn complete_provider_account(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    operation: &'static str,
    request: &IdempotencyRequest,
    id: PaymentProviderAccountId,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        transaction,
        &IdempotencyScope::Store(store_id.as_uuid()),
        operation,
        request,
        200,
        json!({"data":{"id":id.as_uuid()}}),
    )
    .await
}

fn map_provider_account_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error {
        let (code, message) = match database.constraint() {
            Some("provider_accounts_store_provider_key") => (
                "payment_provider_already_configured",
                "the Payment Provider is already configured for this Store",
            ),
            _ => return database_error(error),
        };
        return ApplicationError::Conflict { code, message };
    }
    database_error(error)
}

fn provider_account_not_found(id: PaymentProviderAccountId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "payment_provider_account",
        id: id.as_uuid().to_string(),
    }
}

fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Payment Provider account state"
    ))
}

fn provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_unavailable",
        message: "no enabled Payment Provider account is available",
    }
}

fn payment_order_not_pending() -> ApplicationError {
    ApplicationError::Conflict {
        code: "order_not_pending_payment",
        message: "the Order is not awaiting payment",
    }
}

fn active_attempt_exists() -> ApplicationError {
    ApplicationError::Conflict {
        code: "active_payment_attempt_exists",
        message: "the Order already has an active Payment Attempt",
    }
}

fn provider_reference_mismatch() -> ApplicationError {
    ApplicationError::Conflict {
        code: "provider_reference_mismatch",
        message: "the provider reference does not match the Payment Attempt",
    }
}

fn corrupt_payment_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains an unknown Payment state"
    ))
}

fn corrupt_webhook_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("verified webhook payload is invalid"))
}

fn queue_job_not_found() -> ApplicationError {
    ApplicationError::Conflict {
        code: "queue_lease_lost",
        message: "the queue job is no longer leased by this worker",
    }
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
            ApplicationError::Unavailable {
                service: "postgresql",
                source: error.into(),
            }
        }
        _ => ApplicationError::Unexpected(error.into()),
    }
}
