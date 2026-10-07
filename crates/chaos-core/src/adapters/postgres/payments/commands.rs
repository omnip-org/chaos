use super::events::{
    PaymentEvent, RefundEvent, apply_payment_event, apply_refund_event,
    load_refund_reconciliation_context,
};
use super::repository::*;

use chaos_domain::{
    CurrencyCode,
    payments::{PaymentAttemptStatus, Refund, RefundId, RefundStatus},
    pricing::Money,
    sales::OrderId,
    store::{SalesChannelId, StoreId},
};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApplicationError,
    adapters::postgres::sales::release_order_inventory,
    contracts::{
        AdminActor, CheckoutActor, OrderMetadataContext, PaymentCheckoutDetails, PaymentCommand,
        PaymentCommandKind, PaymentCommandResult, PaymentLineItem, PaymentShippingAddress,
        RefundDetail, ShopperActor,
    },
    error::database_error,
};

#[derive(sqlx::FromRow)]
struct CheckoutResultRow {
    cart_id: Uuid,
    order_status: String,
    payment_status: String,
    client_action: Option<Value>,
}

#[derive(sqlx::FromRow)]
struct RefundOrderRow {
    total_amount_minor: i64,
    currency: String,
    payment_status: String,
    payment_provider_account_id: Uuid,
}

#[derive(sqlx::FromRow)]
struct PaymentCommandContextRow {
    amount_minor: i64,
    currency: String,
    provider_account_id: Uuid,
    credential_secret_reference: String,
    provider_payment_reference: Option<String>,
    shopper_id: Uuid,
    channel_id: Uuid,
    order_number: String,
    order_id: Uuid,
    refund_id: Option<Uuid>,
}

#[derive(sqlx::FromRow)]
struct CheckoutCustomerRow {
    contact_email: Option<String>,
    contact_phone: Option<String>,
    shipping_full_name: Option<String>,
    shipping_address_line1: Option<String>,
    shipping_address_line2: Option<String>,
    shipping_locality: Option<String>,
    shipping_administrative_area: Option<String>,
    shipping_postal_code: Option<String>,
    shipping_country_code: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PaymentLineRow {
    product_title: String,
    variant_title: String,
    sku: Option<String>,
    quantity: i32,
    unit_price_amount_minor: i64,
    image_url: Option<String>,
}

impl PostgresStripeRepository {
    pub(crate) async fn get_order_checkout_payment(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
    ) -> Result<Option<OrderCheckoutPayment>, ApplicationError> {
        let mut transaction = self.begin_shopper(shopper).await?;
        let payment = load_order_checkout_payment(
            &mut transaction,
            &shopper.machine,
            Some(shopper.shopper_id.as_uuid()),
            order_id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(payment)
    }

    pub(crate) async fn get_checkout_payment(
        &self,
        actor: &CheckoutActor,
    ) -> Result<Option<OrderCheckoutPayment>, ApplicationError> {
        let mut transaction = self.begin_machine(actor.machine()).await?;
        let payment =
            load_order_checkout_payment(&mut transaction, actor.machine(), None, actor.order_id())
                .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(payment)
    }

    pub(crate) async fn prepare_checkout_command(
        &self,
        actor: &ShopperActor,
        payment: &OrderCheckoutPayment,
    ) -> Result<PaymentCommand, ApplicationError> {
        let payload = json!({
            "aggregate_id": payment.order_id.as_uuid(),
            "amount_minor": payment.amount_minor,
            "currency": payment.currency.as_str(),
        });
        let mut command = self
            .prepare_payment_command(
                actor.machine.store_id.as_uuid(),
                false,
                Some(&payment.provider),
                &payload,
            )
            .await?;
        command.idempotency_key = checkout_provider_idempotency_key(payment.order_id);
        Ok(command)
    }

    pub(crate) async fn record_checkout_result(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
        result: &PaymentCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if result.provider_object_id.trim().is_empty()
            || result.provider_object_id.chars().count() > 255
        {
            return Err(stripe_invalid_response());
        }
        let client_action = result
            .client_action
            .as_ref()
            .ok_or_else(checkout_client_action_missing)?;
        if client_action.public_key.expose_secret().trim().is_empty()
            || client_action.client_token.expose_secret().trim().is_empty()
        {
            return Err(stripe_invalid_response());
        }
        let actor = &shopper.machine;
        let channel_id = actor.channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let existing = sqlx::query_as::<_, CheckoutResultRow>(
            "SELECT sales_order.cart_id, sales_order.status::text AS order_status, \
                    sales_order.payment_status::text AS payment_status, \
                    source_cart.payment_client_action AS client_action \
             FROM chaos_commerce.orders AS sales_order \
             INNER JOIN chaos_commerce.carts AS source_cart \
               ON source_cart.store_id = sales_order.store_id AND source_cart.id = sales_order.cart_id \
             WHERE sales_order.store_id = $1 AND sales_order.channel_id = $2 \
               AND sales_order.shopper_id = $3 AND sales_order.id = $4 \
             FOR UPDATE OF sales_order, source_cart",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        if let Some(existing_action) = existing.client_action {
            let existing_action = parse_payment_client_action(existing_action)?
                .ok_or_else(checkout_client_action_missing)?;
            if !same_client_action(&existing_action, client_action) {
                return Err(stripe_object_mismatch());
            }
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        }
        if existing.order_status != "pending" || existing.payment_status != "pending" {
            // A payment webhook won the race. Never resurrect a terminal
            // Order with a client action that can no longer be used.
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        }
        let action = payment_client_action_json(client_action);
        let rows = sqlx::query(
            "UPDATE chaos_commerce.carts SET payment_client_action = $3, updated_at = $4 \
             WHERE store_id = $1 AND id = $2 AND status = 'locked'",
        )
        .bind(actor.store_id.as_uuid())
        .bind(existing.cart_id)
        .bind(action)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        if rows != 1 {
            return Err(corrupt_checkout_state());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn fail_checkout_order(
        &self,
        shopper: &ShopperActor,
        order_id: OrderId,
        failure_code: &str,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let actor = &shopper.machine;
        let mut transaction = self.begin_shopper(shopper).await?;
        let row = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT cart_id, status::text FROM chaos_commerce.orders \
             WHERE store_id = $1 AND channel_id = $2 AND shopper_id = $3 AND id = $4 \
             FOR UPDATE",
        )
        .bind(actor.store_id.as_uuid())
        .bind(actor.channel_id.map(SalesChannelId::as_uuid))
        .bind(shopper.shopper_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        if row.1 != "pending" {
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        }
        release_order_inventory(
            &mut transaction,
            actor.store_id.as_uuid(),
            order_id.as_uuid(),
        )
        .await?;
        sqlx::query(
            "UPDATE chaos_commerce.orders SET status = 'cancelled'::chaos_commerce.order_status, \
                    payment_status = 'failed'::chaos_commerce.order_payment_status, \
                    payment_failure_code = $3, updated_at = $4 \
             WHERE store_id = $1 AND id = $2 AND status = 'pending'",
        )
        .bind(actor.store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(normalize_failure_code(failure_code))
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        sqlx::query(
            "UPDATE chaos_commerce.carts SET status = 'abandoned'::chaos_commerce.cart_status, \
                    payment_client_action = NULL, updated_at = $3 \
             WHERE store_id = $1 AND id = $2",
        )
        .bind(actor.store_id.as_uuid())
        .bind(row.0)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn create_refund(
        &self,
        actor: AdminActor,
        store_id: StoreId,
        order_id: OrderId,
        amount_minor: i64,
    ) -> Result<RefundDetail, ApplicationError> {
        let mut transaction = self.begin_admin(&actor).await?;
        let order = sqlx::query_as::<_, RefundOrderRow>(
            "SELECT total_amount_minor, currency::text AS currency, \
                    payment_status::text AS payment_status, payment_provider_account_id \
             FROM chaos_commerce.orders WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        let currency = CurrencyCode::parse(&order.currency)?;
        // The captured amount available to refund against is the Order's
        // total — only an Order that has been paid (in full, or already
        // partially refunded) is eligible for a further refund.
        let payment_status = match order.payment_status.as_str() {
            "paid" | "partially_refunded" => PaymentAttemptStatus::Captured,
            "expired" => PaymentAttemptStatus::Expired,
            _ => PaymentAttemptStatus::Failed,
        };
        // Pending refunds already claim their share of the captured amount,
        // so a second concurrent request cannot double-spend it before the
        // first one confirms via webhook.
        let already_refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_minor), 0)::bigint FROM chaos_commerce.order_refunds \
             WHERE store_id = $1 AND order_id = $2 AND status IN ('pending', 'succeeded')",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let refund = Refund::create(
            order_id,
            payment_status,
            Money::new(order.total_amount_minor, currency),
            Money::new(amount_minor, currency),
            already_refunded,
        )?;
        let id = refund.id();
        sqlx::query(
            "INSERT INTO chaos_commerce.order_refunds \
             (id, store_id, order_id, currency, status, amount_minor, \
              payment_provider_account_id) \
             VALUES ($1, $2, $3, $4, 'pending', $5, $6)",
        )
        .bind(id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(currency.as_str())
        .bind(amount_minor)
        .bind(order.payment_provider_account_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        let detail = RefundDetail {
            id,
            order_id,
            amount_minor,
            currency,
            status: RefundStatus::Pending,
            provider_reference_id: None,
            failure_code: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn process_webhook_job(
        &self,
        store_id: Uuid,
        normalized_event_type: &str,
        provider_account_id: Uuid,
        event_payload: &Value,
        now: OffsetDateTime,
    ) -> Result<Option<RefundReconciliationContext>, ApplicationError> {
        let mut transaction = self.begin_context(None, store_id).await?;
        let failure_code = event_payload
            .get("failure_code")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut reconciliation = None;
        if normalized_event_type.starts_with("payment.") {
            let order_id = event_payload
                .get("order_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(OrderId::from_uuid)
                .ok_or_else(corrupt_webhook_payload)?;
            apply_payment_event(
                &mut transaction,
                PaymentEvent {
                    store_id: StoreId::from_uuid(store_id),
                    order_id,
                    provider_account_id,
                    event_type: normalized_event_type,
                    failure_code,
                    payload: event_payload,
                    now,
                },
            )
            .await?;
        } else if normalized_event_type == "refund.reconcile" {
            let payment_intent = event_payload
                .get("provider_payment_intent")
                .and_then(Value::as_str)
                .filter(|value| value.starts_with("pi_"))
                .ok_or_else(corrupt_webhook_payload)?;
            reconciliation = load_refund_reconciliation_context(
                &mut transaction,
                StoreId::from_uuid(store_id),
                provider_account_id,
                payment_intent,
            )
            .await?;
        } else if normalized_event_type.starts_with("refund.") {
            let stripe_object_id = event_payload
                .get("object")
                .and_then(Value::as_str)
                .ok_or_else(corrupt_webhook_payload)?
                .to_owned();
            let refund_id = event_payload
                .get("refund_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(RefundId::from_uuid);
            apply_refund_event(
                &mut transaction,
                RefundEvent {
                    store_id: StoreId::from_uuid(store_id),
                    refund_id,
                    provider_account_id,
                    event_type: normalized_event_type,
                    provider_reference_id: stripe_object_id,
                    failure_code,
                    payload: event_payload,
                    now,
                },
            )
            .await?;
        } else {
            return Err(corrupt_webhook_payload());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(reconciliation)
    }

    pub(crate) async fn prepare_payment_command(
        &self,
        store_id: Uuid,
        is_refund: bool,
        provider: Option<&str>,
        payload: &Value,
    ) -> Result<PaymentCommand, ApplicationError> {
        let provider = provider.unwrap_or("stripe");
        let aggregate_id = outbox_aggregate_id(payload)?;
        let mut transaction = self.begin_context(None, store_id).await?;
        let context: PaymentCommandContextRow = if is_refund {
            sqlx::query_as(
                "SELECT refund.amount_minor, refund.currency::text AS currency, \
                        account.id AS provider_account_id, account.credential_secret_reference, \
                        sales_order.payment_provider_reference_id AS provider_payment_reference, \
                        sales_order.shopper_id, sales_order.channel_id, \
                        sales_order.order_number, sales_order.id AS order_id, \
                        refund.id AS refund_id \
                 FROM chaos_commerce.order_refunds AS refund \
                 INNER JOIN chaos_commerce.orders AS sales_order \
                   ON sales_order.store_id = refund.store_id AND sales_order.id = refund.order_id \
                 INNER JOIN chaos_integration.provider_accounts AS account \
                   ON account.store_id = refund.store_id \
                  AND account.id = refund.payment_provider_account_id \
                  AND account.capability = 'payment' \
                  AND account.provider = $3 \
                  AND account.enabled \
                 WHERE refund.store_id = $1 AND refund.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL \
                 ORDER BY account.id LIMIT 1",
            )
            .bind(store_id)
            .bind(aggregate_id)
            .bind(provider)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?
        } else {
            sqlx::query_as(
                "SELECT sales_order.subtotal_amount_minor AS amount_minor, \
                        sales_order.currency::text AS currency, \
                        account.id AS provider_account_id, account.credential_secret_reference, \
                        sales_order.payment_provider_reference_id AS provider_payment_reference, \
                        sales_order.shopper_id, sales_order.channel_id, \
                        sales_order.order_number, sales_order.id AS order_id, \
                        NULL::uuid AS refund_id \
                 FROM chaos_commerce.orders AS sales_order \
                 INNER JOIN chaos_integration.provider_accounts AS account \
                   ON account.store_id = sales_order.store_id \
                  AND account.id = sales_order.payment_provider_account_id \
                  AND account.capability = 'payment' \
                  AND account.provider = $3 \
                  AND account.enabled \
                 WHERE sales_order.store_id = $1 AND sales_order.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL \
                 ORDER BY account.id LIMIT 1",
            )
            .bind(store_id)
            .bind(aggregate_id)
            .bind(provider)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?
        };
        let command_amount = context.amount_minor;
        if !is_refund
            && (command_amount != outbox_amount(payload)?
                || context.currency != outbox_currency(payload)?)
        {
            return Err(invalid_outbox_payload());
        }
        if is_refund && command_amount != outbox_amount(payload)? {
            return Err(invalid_outbox_payload());
        }
        if is_refund && context.provider_payment_reference.is_none() {
            return Err(ApplicationError::Conflict {
                code: "payment_provider_reference_missing",
                message: "the Order has no payment provider reference",
            });
        }
        let checkout_details = if !is_refund {
            let order_id = aggregate_id;
            let customer = sqlx::query_as::<_, CheckoutCustomerRow>(
                "SELECT contact_email::text, contact_phone, shipping_full_name, \
                        shipping_address_line1, shipping_address_line2, shipping_locality, \
                        shipping_administrative_area, shipping_postal_code, \
                        NULLIF(btrim(shipping_country_code::text), '') AS shipping_country_code \
                 FROM chaos_commerce.orders WHERE store_id = $1 AND id = $2",
            )
            .bind(store_id)
            .bind(order_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(corrupt_state)?;
            let shipping_address = customer
                .shipping_full_name
                .map(|name| {
                    let Some(line1) = customer.shipping_address_line1 else {
                        return Err(corrupt_state());
                    };
                    let Some(city) = customer.shipping_locality else {
                        return Err(corrupt_state());
                    };
                    let Some(country_code) = customer.shipping_country_code else {
                        return Err(corrupt_state());
                    };
                    Ok(PaymentShippingAddress {
                        name,
                        line1,
                        line2: customer.shipping_address_line2,
                        city,
                        state: customer.shipping_administrative_area,
                        postal_code: customer.shipping_postal_code,
                        country_code,
                    })
                })
                .transpose()?;
            let line_rows = sqlx::query_as::<_, PaymentLineRow>(
                "SELECT product_title, variant_title, sku, quantity, \
                        unit_price_amount_minor, image_url \
                 FROM chaos_commerce.order_lines WHERE store_id = $1 AND order_id = $2 \
                 ORDER BY position",
            )
            .bind(store_id)
            .bind(order_id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
            let line_items = line_rows
                .into_iter()
                .map(|line| {
                    Ok::<_, ApplicationError>(PaymentLineItem {
                        name: if line.variant_title.trim().is_empty() {
                            line.product_title
                        } else {
                            format!("{} — {}", line.product_title, line.variant_title)
                        },
                        sku: line.sku,
                        image_url: line.image_url,
                        quantity: u32::try_from(line.quantity).map_err(unexpected_conversion)?,
                        unit_amount_minor: line.unit_price_amount_minor,
                    })
                })
                .collect::<Result<Vec<_>, ApplicationError>>()?;
            // Shipping policy is read only while creating a provider session.
            // A Cart retry with a stored client action never reaches
            // this path, so editing the Store policy cannot invalidate it.
            let shipping_countries: Vec<String> = sqlx::query_scalar(
                "SELECT country_code::text FROM chaos_commerce.store_shipping_countries \
                 WHERE store_id = $1 AND enabled ORDER BY country_code",
            )
            .bind(store_id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
            if shipping_countries.is_empty() {
                return Err(shipping_countries_unavailable());
            }
            // Shipping rates and destination rules belong to Stripe Checkout.
            // Chaos only stores the address and the provider's final shipping amount.
            let shipping_options = Vec::new();
            Some(PaymentCheckoutDetails {
                customer_email: customer.contact_email,
                customer_phone: customer.contact_phone,
                shipping_address,
                line_items,
                shipping_countries,
                shipping_options,
                automatic_tax: true,
            })
        } else {
            None
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(PaymentCommand {
            provider_account_id: context.provider_account_id,
            kind: if is_refund {
                PaymentCommandKind::CreateRefund
            } else {
                PaymentCommandKind::CreateCheckoutSession
            },
            aggregate_id: context.order_id,
            refund_id: context.refund_id,
            amount_minor: command_amount,
            currency: CurrencyCode::parse(&context.currency)?,
            // Both callers (prepare_checkout_command, create_refund in
            // payments/mod.rs) overwrite this with their own stable
            // identifier immediately after this call returns.
            idempotency_key: aggregate_id.to_string(),
            credential_secret_reference: context.credential_secret_reference,
            provider_payment_reference: context.provider_payment_reference,
            checkout_details,
            order_context: OrderMetadataContext {
                store_id,
                shopper_id: context.shopper_id,
                channel_id: context.channel_id,
                order_number: context.order_number,
            },
        })
    }

    /// Only ever called for a refund command, issued synchronously from
    /// `create_refund` in payments/mod.rs. Checkout Session creation has its
    /// own pair (`prepare_checkout_command`/`record_checkout_result`) and
    /// never reaches here.
    pub(crate) async fn record_payment_result(
        &self,
        store_id: Uuid,
        payload: &Value,
        result: &PaymentCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if result.provider_object_id.trim().is_empty()
            || result.provider_object_id.chars().count() > 255
        {
            return Err(stripe_invalid_response());
        }
        let aggregate_id = outbox_aggregate_id(payload)?;
        let mut transaction = self.begin_context(None, store_id).await?;
        let rows = sqlx::query(
            "UPDATE chaos_commerce.order_refunds \
             SET payment_provider_reference_id = COALESCE(payment_provider_reference_id, $3), \
                 updated_at = CASE WHEN payment_provider_reference_id IS NULL THEN $4 ELSE updated_at END \
             WHERE store_id = $1 AND id = $2 \
               AND (payment_provider_reference_id IS NULL OR payment_provider_reference_id = $3)",
        )
        .bind(store_id)
        .bind(aggregate_id)
        .bind(&result.provider_object_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        if rows != 1 {
            return Err(stripe_object_mismatch());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }
}

fn checkout_client_action_missing() -> ApplicationError {
    ApplicationError::Unavailable {
        service: "payment_client_action",
        source: anyhow::anyhow!("the Payment provider returned no client action"),
    }
}

fn shipping_countries_unavailable() -> ApplicationError {
    ApplicationError::Conflict {
        code: "shipping_countries_unavailable",
        message: "the Store has no enabled shipping destinations",
    }
}

fn normalize_failure_code(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "checkout_failed".into();
    }
    value.chars().take(2000).collect()
}
