// Storefront ownership checks, parsing, workflow transitions, and shared errors.

async fn ensure_cart_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    cart_id: CartId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.carts \
         WHERE store_id = $1 AND sales_channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(cart_id.as_uuid())
    .bind(shopper_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if owned {
        Ok(())
    } else {
        Err(cart_not_found(cart_id))
    }
}

async fn ensure_checkout_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    checkout_id: CheckoutId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.checkouts \
         WHERE store_id = $1 AND sales_channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(checkout_id.as_uuid())
    .bind(shopper_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if owned {
        Ok(())
    } else {
        Err(checkout_not_found(checkout_id))
    }
}

async fn ensure_order_owner(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    order_id: OrderId,
    shopper_id: ShopperId,
) -> Result<(), ApplicationError> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commerce.orders \
         WHERE store_id = $1 AND sales_channel_id = $2 \
           AND id = $3 AND shopper_id = $4)",
    )
    .bind(actor.store_id.as_uuid())
    .bind(actor.sales_channel_id.map(SalesChannelId::as_uuid))
    .bind(order_id.as_uuid())
    .bind(shopper_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    if owned {
        Ok(())
    } else {
        Err(order_not_found(order_id))
    }
}

async fn reserve(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
) -> Result<Option<Value>, ApplicationError> {
    idempotency::reserve(transaction, scope, operation, request).await
}

async fn complete(
    transaction: &mut Transaction<'static, Postgres>,
    scope: &IdempotencyScope,
    operation: &'static str,
    request: &IdempotencyRequest,
    status: i16,
    snapshot: Value,
) -> Result<(), ApplicationError> {
    idempotency::complete(transaction, scope, operation, request, status, snapshot).await
}

fn require_channel(actor: &MachineActor) -> Result<SalesChannelId, ApplicationError> {
    actor.sales_channel_id.ok_or(ApplicationError::Forbidden)
}

fn parse_currency(value: &str) -> Result<CurrencyCode, ApplicationError> {
    CurrencyCode::parse(value).map_err(ApplicationError::from)
}

fn parse_locale(value: &str) -> Result<Locale, ApplicationError> {
    Locale::parse(value).map_err(Into::into)
}

fn default_locale_snapshot() -> String {
    "en-US".into()
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
        "invalid sales idempotency snapshot: {error}"
    ))
}

fn unexpected_conversion(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn cart_not_found(cart_id: CartId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "cart",
        id: cart_id.as_uuid().to_string(),
    }
}

fn checkout_not_found(checkout_id: CheckoutId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "checkout",
        id: checkout_id.as_uuid().to_string(),
    }
}

fn invalid_shipping_selection() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "shipping_service_id",
            reason: "must reference an active service for the Cart currency and destination".into(),
        }],
    }
}

fn invalid_promotion_code() -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "promotion_code",
            reason: "must reference an active and eligible code for the Cart".into(),
        }],
    }
}

fn tax_rule_unavailable(country_code: &str) -> ApplicationError {
    ApplicationError::Validation {
        violations: vec![chaos_domain::FieldViolation {
            field: "tax_rule",
            reason: format!("no active Tax Rule is configured for destination {country_code}"),
        }],
    }
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

fn checkout_not_pending() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_not_pending",
        message: "the Checkout is no longer pending",
    }
}

fn checkout_already_pending() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_already_pending",
        message: "the Cart already has a pending Checkout",
    }
}

fn checkout_expired() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_expired",
        message: "the Checkout has expired",
    }
}

fn checkout_expiry_lease_lost() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_expiry_lease_lost",
        message: "the Checkout expiry lease is no longer owned by this worker",
    }
}

fn cart_not_active() -> ApplicationError {
    ApplicationError::Conflict {
        code: "cart_not_active",
        message: "the Cart is no longer active",
    }
}

fn price_context_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "price_context_unavailable",
        message: "no active Price List is available for the requested currency",
    }
}

fn variant_unavailable(variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "product_variant",
        id: variant_id.as_uuid().to_string(),
    }
}

fn cart_line_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "cart_line_unavailable",
        message: "one or more Cart lines are no longer published and priced",
    }
}

fn insufficient_inventory(_variant_id: ProductVariantId) -> ApplicationError {
    ApplicationError::Conflict {
        code: "insufficient_inventory",
        message: "one or more Cart lines exceed available inventory",
    }
}

fn corrupt_sales_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("database contains an unknown sales state"))
}

fn database_error(error: sqlx::Error) -> ApplicationError {
    eprintln!("DEBUG SQL ERROR: {error}");
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
