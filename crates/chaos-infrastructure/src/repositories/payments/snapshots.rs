// Idempotency snapshots and payment/refund response reconstruction.

#[derive(Serialize, Deserialize)]
struct AttemptSnapshot {
    id: Uuid,
    order_id: Uuid,
    amount_minor: i64,
    currency: String,
    status: String,
    stripe_checkout_session_id: Option<String>,
    failure_code: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Deserialize)]
struct RefundSnapshot {
    id: Uuid,
    payment_attempt_id: Uuid,
    amount_minor: i64,
    currency: String,
    status: String,
    stripe_refund_id: Option<String>,
    failure_code: Option<String>,
    created_at: String,
    updated_at: String,
}

fn attempt_snapshot(detail: &PaymentAttemptDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(AttemptSnapshot {
        id: detail.id.as_uuid(),
        order_id: detail.order_id.as_uuid(),
        amount_minor: detail.amount_minor,
        currency: detail.currency.as_str().into(),
        status: detail.status.as_str().into(),
        stripe_checkout_session_id: detail.stripe_checkout_session_id.clone(),
        failure_code: detail.failure_code.clone(),
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_attempt(value: Value) -> Result<PaymentAttemptDetail, ApplicationError> {
    let value: AttemptSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(PaymentAttemptDetail {
        id: PaymentAttemptId::from_uuid(value.id),
        order_id: OrderId::from_uuid(value.order_id),
        amount_minor: value.amount_minor,
        currency: CurrencyCode::parse(&value.currency)?,
        status: PaymentAttemptStatus::parse(&value.status).ok_or_else(corrupt_payment_state)?,
        stripe_checkout_session_id: value.stripe_checkout_session_id,
        failure_code: value.failure_code,
        created_at: parse_time(&value.created_at)?,
        updated_at: parse_time(&value.updated_at)?,
    })
}

fn refund_snapshot(detail: &RefundDetail) -> Result<Value, ApplicationError> {
    serde_json::to_value(RefundSnapshot {
        id: detail.id.as_uuid(),
        payment_attempt_id: detail.payment_attempt_id.as_uuid(),
        amount_minor: detail.amount_minor,
        currency: detail.currency.as_str().into(),
        status: detail.status.as_str().into(),
        stripe_refund_id: detail.stripe_refund_id.clone(),
        failure_code: detail.failure_code.clone(),
        created_at: format_time(detail.created_at)?,
        updated_at: format_time(detail.updated_at)?,
    })
    .map_err(|error| ApplicationError::Unexpected(error.into()))
}

fn replay_refund(value: Value) -> Result<RefundDetail, ApplicationError> {
    let value: RefundSnapshot = serde_json::from_value(value).map_err(invalid_snapshot)?;
    Ok(RefundDetail {
        id: RefundId::from_uuid(value.id),
        payment_attempt_id: PaymentAttemptId::from_uuid(value.payment_attempt_id),
        amount_minor: value.amount_minor,
        currency: CurrencyCode::parse(&value.currency)?,
        status: RefundStatus::parse(&value.status).ok_or_else(corrupt_payment_state)?,
        stripe_refund_id: value.stripe_refund_id,
        failure_code: value.failure_code,
        created_at: parse_time(&value.created_at)?,
        updated_at: parse_time(&value.updated_at)?,
    })
}
