// Payment repository core wiring, provider implementations, and shared imports.

use async_trait::async_trait;
use crate::{
    ApplicationError,
    adapters::postgres::database::{
        generate_order_tracking_capability, ORDER_TRACKING_TOKEN_LIFETIME,
    },
    error::database_error,
    contracts::{
        AdminActor, CheckoutAttemptDetail, MachineActor, OrderMetadataContext,
        PaymentCheckoutDetails, PaymentClientAction, PaymentLineItem,
        StripeAccountConfiguration,
        StripeAccountDetail, StripeAccountPage,
        PaymentRefundObservation, PaymentRefundStatus, PaymentShippingAddress, PaymentCommand,
        PaymentCommandKind, PaymentCommandResult, StripeWebhookConfiguration,
        StripeWebhookConfigurationRepository, QueueJob, RefundDetail, ShopperActor,
    },
    store::StoreActor,
};
use chaos_domain::{
    CurrencyCode,
    payments::{
        CheckoutAttemptId, CheckoutAttemptStatus, PaymentAttemptStatus, Refund, RefundId,
        RefundStatus,
    },
    pricing::Money,
    sales::{CartId, Order, OrderId, OrderStatus},
    stripe::{PaymentSecretReference, StripeAccount, StripeAccountId},
    store::StoreId,
};
use serde_json::{Value, json};
use secrecy::{ExposeSecret, SecretString};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::adapters::postgres::{
    analytics::{
        AnalyticsEventToAppend, append_event, load_checkout_attribution, merge_attribution,
        merge_order_identity,
    },
    sales::{consume_order_inventory, release_order_inventory},
};

type ProviderAccountRow = (
    Uuid,
    String,
    bool,
    OffsetDateTime,
    OffsetDateTime,
);

#[derive(Clone)]
pub(crate) struct RefundReconciliationContext {
    pub store_id: StoreId,
    pub order_id: OrderId,
    pub provider_account_id: Uuid,
    pub credential_secret_reference: String,
    pub payment_provider_reference: String,
}

#[derive(Clone)]
pub struct PostgresStripeRepository {
    pool: PgPool,
}

pub(crate) struct CheckoutAttemptPayment {
    pub detail: CheckoutAttemptDetail,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub provider: String,
    pub provider_idempotency_key: Uuid,
    pub provider_public_key: Option<String>,
    pub provider_client_secret: Option<String>,
    pub return_url: String,
}

#[derive(sqlx::FromRow)]
struct CheckoutAttemptPaymentRow {
    id: Uuid,
    order_id: Uuid,
    source_cart_id: Uuid,
    successor_cart_id: Uuid,
    amount_minor: i64,
    currency: String,
    status: String,
    expires_at: OffsetDateTime,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    provider: String,
    provider_idempotency_key: Uuid,
    provider_public_key: Option<String>,
    provider_client_secret: Option<String>,
    return_url: String,
}

impl CheckoutAttemptPayment {
    pub(crate) fn client_action(&self) -> Result<Option<PaymentClientAction>, ApplicationError> {
        match (
            self.provider_public_key.as_deref(),
            self.provider_client_secret.as_deref(),
        ) {
            (Some(public_key), Some(client_token)) => Ok(Some(PaymentClientAction {
                kind: "mount_embedded_checkout",
                public_key: SecretString::from(public_key.to_owned()),
                client_token: SecretString::from(client_token.to_owned()),
            })),
            (None, None) => Ok(None),
            _ => Err(stripe_invalid_response()),
        }
    }
}

impl PostgresStripeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin_machine(
        &self,
        actor: &MachineActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        self.begin_context(None, actor.store_id.as_uuid()).await
    }

    async fn begin_shopper(
        &self,
        shopper: &ShopperActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.begin_machine(&shopper.machine).await?;
        set_config(
            &mut transaction,
            "app.shopper_id",
            shopper.shopper_id.as_uuid(),
        )
        .await?;
        Ok(transaction)
    }

    async fn begin_human(
        &self,
        actor: StoreActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        self.begin_context(Some(actor.user_id().as_uuid()), actor.store_id().as_uuid())
            .await
    }

    async fn begin_admin(
        &self,
        actor: &AdminActor,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        self.begin_context(
            actor.audit_user_id().map(|id| id.as_uuid()),
            actor.store_id().as_uuid(),
        )
        .await
    }

    async fn begin_context(
        &self,
        user_id: Option<Uuid>,
        store_id: Uuid,
    ) -> Result<Transaction<'static, Postgres>, ApplicationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        if let Some(user_id) = user_id {
            set_config(&mut transaction, "app.user_id", user_id).await?;
        }
        set_config(&mut transaction, "app.store_id", store_id).await?;
        Ok(transaction)
    }
}

async fn load_checkout_attempt_payment(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    shopper_id: Uuid,
    attempt_id: CheckoutAttemptId,
) -> Result<Option<CheckoutAttemptPayment>, ApplicationError> {
    let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
    let row = sqlx::query_as::<_, CheckoutAttemptPaymentRow>(
        "SELECT attempt.id AS id, attempt.order_id AS order_id, \
                attempt.source_cart_id AS source_cart_id, attempt.successor_cart_id AS successor_cart_id, \
                sales_order.subtotal_amount_minor AS amount_minor, sales_order.currency::text AS currency, \
                attempt.status::text AS status, attempt.expires_at AS expires_at, \
                attempt.created_at AS created_at, attempt.updated_at AS updated_at, \
                account.provider::text AS provider, \
                attempt.provider_idempotency_key AS provider_idempotency_key, \
                attempt.provider_public_key AS provider_public_key, \
                attempt.provider_client_secret AS provider_client_secret, attempt.return_url AS return_url \
         FROM commerce.checkout_attempts AS attempt \
         INNER JOIN commerce.orders AS sales_order \
           ON sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
         INNER JOIN integration.provider_accounts AS account \
           ON account.store_id = attempt.store_id \
          AND account.id = attempt.payment_provider_account_id \
         WHERE attempt.store_id = $1 AND attempt.sales_channel_id = $2 \
           AND attempt.shopper_id = $3 AND attempt.id = $4",
    )
    .bind(actor.store_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(shopper_id)
    .bind(attempt_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        let status = CheckoutAttemptStatus::parse(&row.status).ok_or_else(corrupt_checkout_state)?;
        Ok(CheckoutAttemptPayment {
            detail: CheckoutAttemptDetail {
                id: CheckoutAttemptId::from_uuid(row.id),
                order_id: OrderId::from_uuid(row.order_id),
                source_cart_id: CartId::from_uuid(row.source_cart_id),
                successor_cart_id: CartId::from_uuid(row.successor_cart_id),
                status,
                expires_at: row.expires_at,
                created_at: row.created_at,
                updated_at: row.updated_at,
            },
            amount_minor: row.amount_minor,
            currency: CurrencyCode::parse(&row.currency)?,
            provider: row.provider,
            provider_idempotency_key: row.provider_idempotency_key,
            provider_public_key: row.provider_public_key,
            provider_client_secret: row.provider_client_secret,
            return_url: row.return_url,
        })
    })
    .transpose()
}

async fn load_order_analytics_items(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: Uuid,
    order_id: Uuid,
) -> Result<Vec<Value>, ApplicationError> {
    let rows: Vec<(Uuid, Uuid, i32, i64)> = sqlx::query_as(
        "SELECT product_id, product_variant_id, quantity, unit_price_amount_minor
           FROM commerce.order_lines
          WHERE store_id = $1 AND order_id = $2
          ORDER BY position",
    )
    .bind(store_id)
    .bind(order_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(database_error)?;
    rows.into_iter()
        .map(|(product_id, product_variant_id, quantity, price_minor)| {
            Ok(json!({
                "product_id": product_id,
                "product_variant_id": product_variant_id,
                "quantity": i64::from(quantity),
                "price_minor": price_minor,
            }))
        })
        .collect()
}

// Shared transaction helpers and provider account reconstruction.

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

fn unexpected_conversion(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Unexpected(error.into())
}

fn outbox_aggregate_id(job: &QueueJob) -> Result<Uuid, ApplicationError> {
    job.payload
        .get("aggregate_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
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

fn stripe_invalid_response() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "stripe",
        source: anyhow::anyhow!("Stripe returned an invalid object reference"),
    }
}

fn order_not_found(order_id: OrderId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "order",
        id: order_id.as_uuid().to_string(),
    }
}

async fn load_stripe_account(
    transaction: &mut Transaction<'static, Postgres>,
    store_id: StoreId,
    id: StripeAccountId,
) -> Result<Option<StripeAccountDetail>, ApplicationError> {
    sqlx::query_as::<_, ProviderAccountRow>(
        "SELECT id, display_name, \
                credential_secret_reference IS NOT NULL AND webhook_secret_reference IS NOT NULL, \
                created_at, updated_at FROM integration.provider_accounts \
         WHERE store_id = $1 AND id = $2 AND capability = 'payment' AND provider = 'stripe'",
    )
    .bind(store_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .map(stripe_account_detail)
    .transpose()
}

fn stripe_account_detail(
    row: ProviderAccountRow,
) -> Result<StripeAccountDetail, ApplicationError> {
    Ok(StripeAccountDetail {
        account: StripeAccount::rehydrate(
            StripeAccountId::from_uuid(row.0),
            row.1,
        )?,
        credentials_configured: row.2,
        created_at: row.3,
        updated_at: row.4,
    })
}

fn map_provider_account_write_error(error: sqlx::Error) -> ApplicationError {
    if let sqlx::Error::Database(database) = &error {
        let (code, message) = match database.constraint() {
            Some("provider_accounts_store_capability_provider_key") => (
                "payment_provider_already_configured",
                "the Payment Provider is already configured for this Store",
            ),
            _ => return database_error(error),
        };
        return ApplicationError::Conflict { code, message };
    }
    database_error(error)
}

fn provider_account_not_found(id: StripeAccountId) -> ApplicationError {
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

fn corrupt_checkout_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains invalid Checkout Attempt state"
    ))
}

fn provider_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_provider_unavailable",
        message: "no configured Payment Provider account is available",
    }
}

fn stripe_object_mismatch() -> ApplicationError {
    ApplicationError::Conflict {
        code: "stripe_object_mismatch",
        message: "the Stripe object does not match the Payment Attempt",
    }
}

fn stripe_currency_mismatch() -> ApplicationError {
    ApplicationError::Conflict {
        code: "stripe_currency_mismatch",
        message: "the Stripe currency does not match the Payment Attempt",
    }
}

fn corrupt_payment_state() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!(
        "database contains an unknown Payment state"
    ))
}

fn payment_event_out_of_order() -> ApplicationError {
    ApplicationError::Conflict {
        code: "payment_event_out_of_order",
        message: "a payment capture arrived after the Order was cancelled or failed",
    }
}

fn corrupt_webhook_payload() -> ApplicationError {
    ApplicationError::Unexpected(anyhow::anyhow!("verified webhook payload is invalid"))
}
