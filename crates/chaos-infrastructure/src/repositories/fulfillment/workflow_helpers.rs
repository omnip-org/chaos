// Fulfillment event parsing, state transitions, validation, and shared errors.

async fn reserve(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Value>, ApplicationError> {
    idempotency::reserve(tx, &IdempotencyScope::Store(account_id), operation, request).await
}

async fn complete(
    tx: &mut Transaction<'static, Postgres>,
    account_id: Uuid,
    operation: &'static str,
    request: &IdempotencyRequest,
    snapshot: Value,
) -> Result<(), ApplicationError> {
    idempotency::complete(
        tx,
        &IdempotencyScope::Store(account_id),
        operation,
        request,
        200,
        snapshot,
    )
    .await
}

fn require_tracking(
    carrier: Option<&str>,
    tracking_number: Option<&str>,
) -> Result<(), ApplicationError> {
    match (carrier.map(str::trim), tracking_number.map(str::trim)) {
        (Some(carrier), Some(number)) if !carrier.is_empty() && !number.is_empty() => Ok(()),
        _ => Err(validation(
            "tracking",
            "carrier and tracking_number are required when shipping",
        )),
    }
}

fn fulfillment_not_found(id: FulfillmentId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "fulfillment",
        id: id.as_uuid().to_string(),
    }
}

fn return_not_found(id: ReturnId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "return",
        id: id.as_uuid().to_string(),
    }
}

fn invalid_target() -> ApplicationError {
    ApplicationError::Conflict {
        code: "invalid_transition_target",
        message: "the requested target status is not an operation",
    }
}

fn validation(field: &'static str, reason: &'static str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![FieldViolation {
            field,
            reason: reason.into(),
        }],
    }
}

fn refund_amount_overflow() -> ApplicationError {
    validation(
        "refund_amount_minor",
        "sum exceeds the supported amount range",
    )
}

fn corrupt_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains an unknown fulfillment state"
    ))
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
        "invalid fulfillment idempotency snapshot: {error}"
    ))
}

fn unexpected_conversion(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    eprintln!("DEBUG FULFILLMENT SQL ERROR: {error}");
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
