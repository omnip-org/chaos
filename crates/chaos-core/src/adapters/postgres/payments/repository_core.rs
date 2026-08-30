// Payment repository core wiring, provider implementations, and shared imports.

use async_trait::async_trait;
use crate::{
    ApplicationError,
    adapters::postgres::database::{
        generate_order_tracking_capability, ORDER_TRACKING_TOKEN_LIFETIME,
    },
    error::database_error,
    contracts::{
        AdminActor, MachineActor, OrderMetadataContext, PendingPaymentOrder,
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
        PaymentAttemptStatus, Refund, RefundId, RefundStatus,
    },
    pricing::Money,
    sales::{CartId, Order, OrderId, OrderStatus},
    stripe::{PaymentSecretReference, StripeAccount, StripeAccountId},
    store::{SalesChannelId, StoreId},
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

pub(crate) struct OrderCheckoutPayment {
    pub order_id: OrderId,
    pub source_cart_id: CartId,
    pub amount_minor: i64,
    pub currency: CurrencyCode,
    pub provider: String,
    pub order_status: String,
    pub payment_status: String,
    pub client_action: Option<PaymentClientAction>,
}

#[derive(sqlx::FromRow)]
struct OrderCheckoutPaymentRow {
    id: Uuid,
    cart_id: Uuid,
    amount_minor: i64,
    currency: String,
    order_status: String,
    payment_status: String,
    provider: String,
    client_action: Option<Value>,
}

impl OrderCheckoutPaymentRow {
    fn into_payment(self) -> Result<OrderCheckoutPayment, ApplicationError> {
        let client_action = self
            .client_action
            .map(parse_payment_client_action)
            .transpose()?
            .flatten();
        Ok(OrderCheckoutPayment {
            order_id: OrderId::from_uuid(self.id),
            source_cart_id: CartId::from_uuid(self.cart_id),
            amount_minor: self.amount_minor,
            currency: CurrencyCode::parse(&self.currency)?,
            provider: self.provider,
            order_status: self.order_status,
            payment_status: self.payment_status,
            client_action,
        })
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

async fn load_order_checkout_payment(
    transaction: &mut Transaction<'static, Postgres>,
    actor: &MachineActor,
    shopper_id: Uuid,
    order_id: OrderId,
) -> Result<Option<OrderCheckoutPayment>, ApplicationError> {
    let channel_id = actor.channel_id.ok_or(ApplicationError::Forbidden)?;
    let row = sqlx::query_as::<_, OrderCheckoutPaymentRow>(
        "SELECT sales_order.id AS id, sales_order.cart_id AS cart_id, \
                sales_order.subtotal_amount_minor AS amount_minor, \
                sales_order.currency::text AS currency, \
                sales_order.status::text AS order_status, \
                sales_order.payment_status::text AS payment_status, \
                account.provider::text AS provider, \
                source_cart.payment_client_action AS client_action \
         FROM commerce.orders AS sales_order \
         INNER JOIN commerce.carts AS source_cart \
           ON source_cart.store_id = sales_order.store_id AND source_cart.id = sales_order.cart_id \
         INNER JOIN integration.provider_accounts AS account \
           ON account.store_id = sales_order.store_id \
          AND account.id = sales_order.payment_provider_account_id \
         WHERE sales_order.store_id = $1 AND sales_order.channel_id = $2 \
           AND sales_order.shopper_id = $3 AND sales_order.id = $4",
    )
    .bind(actor.store_id.as_uuid())
    .bind(channel_id.as_uuid())
    .bind(shopper_id)
    .bind(order_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    row.map(OrderCheckoutPaymentRow::into_payment).transpose()
}

fn parse_payment_client_action(value: Value) -> Result<Option<PaymentClientAction>, ApplicationError> {
    let Some(object) = value.as_object() else {
        return Err(corrupt_checkout_state());
    };
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return Err(corrupt_checkout_state());
    };
    if kind != "mount_embedded_checkout" {
        return Err(corrupt_checkout_state());
    }
    let Some(public_key) = object.get("public_key").and_then(Value::as_str) else {
        return Err(corrupt_checkout_state());
    };
    let Some(client_token) = object.get("client_token").and_then(Value::as_str) else {
        return Err(corrupt_checkout_state());
    };
    if public_key.trim().is_empty() || client_token.trim().is_empty() {
        return Err(corrupt_checkout_state());
    }
    Ok(Some(PaymentClientAction {
        kind: "mount_embedded_checkout",
        public_key: SecretString::from(public_key.to_owned()),
        client_token: SecretString::from(client_token.to_owned()),
    }))
}

fn payment_client_action_json(action: &PaymentClientAction) -> Value {
    json!({
        "type": action.kind,
        "public_key": action.public_key.expose_secret(),
        "client_token": action.client_token.expose_secret(),
    })
}

fn same_client_action(left: &PaymentClientAction, right: &PaymentClientAction) -> bool {
    left.kind == right.kind
        && left.public_key.expose_secret() == right.public_key.expose_secret()
        && left.client_token.expose_secret() == right.client_token.expose_secret()
}

pub(crate) fn checkout_provider_idempotency_key(order_id: OrderId) -> String {
    format!("checkout:{}", order_id.as_uuid())
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
        "database contains invalid payment client action state"
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

fn stripe_amount_mismatch() -> ApplicationError {
    ApplicationError::Conflict {
        code: "stripe_amount_mismatch",
        message: "the Stripe amount does not match the Checkout breakdown",
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
