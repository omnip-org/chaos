// Order-centric checkout handoff and payment state updates.

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
            shopper.shopper_id.as_uuid(),
            order_id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(payment)
    }

    pub(crate) async fn prepare_checkout_command(
        &self,
        actor: &ShopperActor,
        payment: &OrderCheckoutPayment,
        return_url: &str,
    ) -> Result<PaymentCommand, ApplicationError> {
        let return_url = checkout_return_url(return_url, payment.order_id)?;
        let job = direct_checkout_job(actor, payment, &return_url);
        let mut command = self.prepare_payment_command(&job).await?;
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
        let existing = sqlx::query_as::<_, (Uuid, String, String, Option<Value>)>(
            "SELECT sales_order.cart_id, sales_order.status::text, \
                    sales_order.payment_status::text, source_cart.payment_client_action \
             FROM commerce.orders AS sales_order \
             INNER JOIN commerce.carts AS source_cart \
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
        if let Some(existing_action) = existing.3 {
            let existing_action = parse_payment_client_action(existing_action)?
                .ok_or_else(checkout_client_action_missing)?;
            if !same_client_action(&existing_action, client_action) {
                return Err(stripe_object_mismatch());
            }
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        }
        if existing.1 != "pending" || existing.2 != "pending" {
            // A payment webhook won the race. Never resurrect a terminal
            // Order with a client action that can no longer be used.
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        }
        let action = payment_client_action_json(client_action);
        let rows = sqlx::query(
            "UPDATE commerce.carts SET payment_client_action = $3, updated_at = $4 \
             WHERE store_id = $1 AND id = $2 AND status = 'locked'",
        )
        .bind(actor.store_id.as_uuid())
        .bind(existing.0)
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
            "SELECT cart_id, status::text FROM commerce.orders \
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
        release_order_inventory(&mut transaction, actor.store_id.as_uuid(), order_id.as_uuid())
            .await?;
        sqlx::query(
            "UPDATE commerce.orders SET status = 'cancelled'::commerce.order_status, \
                    payment_status = 'failed'::commerce.order_payment_status, \
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
            "UPDATE commerce.carts SET status = 'abandoned'::commerce.cart_status, \
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
        let row = sqlx::query_as::<_, (i64, String, String, Uuid)>(
            "SELECT total_amount_minor, currency::text, payment_status::text, \
                    payment_provider_account_id \
             FROM commerce.orders WHERE store_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| order_not_found(order_id))?;
        let currency = CurrencyCode::parse(&row.1)?;
        // The captured amount available to refund against is the Order's
        // total — only an Order that has been paid (in full, or already
        // partially refunded) is eligible for a further refund.
        let payment_status = match row.2.as_str() {
            "paid" | "partially_refunded" => PaymentAttemptStatus::Captured,
            "expired" => PaymentAttemptStatus::Expired,
            _ => PaymentAttemptStatus::Failed,
        };
        // Pending refunds already claim their share of the captured amount,
        // so a second concurrent request cannot double-spend it before the
        // first one confirms via webhook.
        let already_refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_minor), 0)::bigint FROM commerce.order_refunds \
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
            Money::new(row.0, currency),
            Money::new(amount_minor, currency),
            already_refunded,
        )?;
        let id = refund.id();
        sqlx::query(
            "INSERT INTO commerce.order_refunds \
             (id, store_id, order_id, currency, status, amount_minor, \
              payment_provider_account_id) \
             VALUES ($1, $2, $3, $4, 'pending', $5, $6)",
        )
        .bind(id.as_uuid())
        .bind(store_id.as_uuid())
        .bind(order_id.as_uuid())
        .bind(currency.as_str())
        .bind(amount_minor)
        .bind(row.3)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        insert_outbox(
            &mut transaction,
            store_id,
            "refund",
            id.as_uuid(),
            "refund.create_requested",
            amount_minor,
            currency,
            None,
        )
        .await?;
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
        job: &QueueJob,
        now: OffsetDateTime,
    ) -> Result<Option<RefundReconciliationContext>, ApplicationError> {
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let failure_code = job
            .payload
            .get("failure_code")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let normalized_event_type = job
            .normalized_event_type
            .as_deref()
            .ok_or_else(corrupt_webhook_payload)?;
        let provider_account_id: Uuid = sqlx::query_scalar(
            "SELECT provider_account_id FROM integration.provider_webhook_inbox WHERE id = $1",
        )
        .bind(job.id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let mut reconciliation = None;
        let resolved_order_id = if normalized_event_type.starts_with("payment.") {
            let order_id = job
                .payload
                .get("order_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(OrderId::from_uuid)
                .ok_or_else(corrupt_webhook_payload)?;
            Some(
                apply_payment_event(
                    &mut transaction,
                    StoreId::from_uuid(job.store_id),
                    order_id,
                    provider_account_id,
                    normalized_event_type,
                    failure_code,
                    &job.payload,
                    now,
                )
                .await?,
            )
        } else if normalized_event_type == "refund.reconcile" {
            let payment_intent = job
                .payload
                .get("provider_payment_intent")
                .and_then(Value::as_str)
                .filter(|value| value.starts_with("pi_"))
                .ok_or_else(corrupt_webhook_payload)?;
            reconciliation = load_refund_reconciliation_context(
                &mut transaction,
                StoreId::from_uuid(job.store_id),
                provider_account_id,
                payment_intent,
            )
            .await?;
            reconciliation.as_ref().map(|context| context.order_id)
        } else if normalized_event_type.starts_with("refund.") {
            let stripe_object_id = job
                .payload
                .get("object")
                .and_then(Value::as_str)
                .ok_or_else(corrupt_webhook_payload)?
                .to_owned();
            let refund_id = job
                .payload
                .get("refund_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(RefundId::from_uuid);
            Some(
                apply_refund_event(
                    &mut transaction,
                    StoreId::from_uuid(job.store_id),
                    refund_id,
                    provider_account_id,
                    normalized_event_type,
                    stripe_object_id,
                    failure_code,
                    &job.payload,
                    now,
                )
                .await?,
            )
        } else {
            return Err(corrupt_webhook_payload());
        };
        if let Some(order_id) = resolved_order_id {
            let updated: bool = sqlx::query_scalar(
                "SELECT integration.set_provider_webhook_aggregate($1, 'order', $2)",
            )
            .bind(job.id)
            .bind(order_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await
            .map_err(database_error)?;
            if !updated {
                return Err(corrupt_webhook_payload());
            }
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(reconciliation)
    }

    pub(crate) async fn prepare_payment_command(
        &self,
        job: &QueueJob,
    ) -> Result<PaymentCommand, ApplicationError> {
        let provider = job.provider.as_deref().unwrap_or("stripe");
        let aggregate_id = outbox_aggregate_id(job)?;
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let internal_event_type = job
            .internal_event_type
            .as_deref()
            .ok_or_else(invalid_outbox_payload)?;
        if !matches!(
            internal_event_type,
            "payment.checkout_session" | "refund.create_requested"
        ) {
            return Err(invalid_outbox_payload());
        }
        let is_refund = internal_event_type == "refund.create_requested";
        type ContextRow = (
            i64,
            String,
            Uuid,
            String,
            Option<String>,
            Uuid,
            Uuid,
            String,
            Uuid,
            Option<Uuid>,
        );
        let row: ContextRow = if is_refund {
            sqlx::query_as(
                "SELECT refund.amount_minor, refund.currency::text, \
                        account.id, account.credential_secret_reference, \
                        sales_order.payment_provider_reference_id, \
                        sales_order.shopper_id, sales_order.channel_id, \
                        sales_order.order_number, sales_order.id, refund.id \
                 FROM commerce.order_refunds AS refund \
                 INNER JOIN commerce.orders AS sales_order \
                   ON sales_order.store_id = refund.store_id AND sales_order.id = refund.order_id \
                 INNER JOIN integration.provider_accounts AS account \
                   ON account.store_id = refund.store_id \
                  AND account.id = refund.payment_provider_account_id \
                  AND account.capability = 'payment' \
                  AND account.provider = $3 \
                  AND account.enabled \
                 WHERE refund.store_id = $1 AND refund.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL \
                 ORDER BY account.id LIMIT 1",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(provider)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?
        } else {
            sqlx::query_as(
                "SELECT sales_order.subtotal_amount_minor, sales_order.currency::text, \
                        account.id, account.credential_secret_reference, \
                        sales_order.payment_provider_reference_id, \
                        sales_order.shopper_id, sales_order.channel_id, \
                        sales_order.order_number, sales_order.id, NULL::uuid \
                 FROM commerce.orders AS sales_order \
                 INNER JOIN integration.provider_accounts AS account \
                   ON account.store_id = sales_order.store_id \
                  AND account.id = sales_order.payment_provider_account_id \
                  AND account.capability = 'payment' \
                  AND account.provider = $3 \
                  AND account.enabled \
                 WHERE sales_order.store_id = $1 AND sales_order.id = $2 \
                   AND account.credential_secret_reference IS NOT NULL \
                 ORDER BY account.id LIMIT 1",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(provider)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(provider_unavailable)?
        };
        let command_amount = row.0;
        if !is_refund && (command_amount != outbox_amount(job)? || row.1 != outbox_currency(job)?) {
            return Err(invalid_outbox_payload());
        }
        if is_refund && command_amount != outbox_amount(job)? {
            return Err(invalid_outbox_payload());
        }
        if is_refund && row.4.is_none() {
            return Err(ApplicationError::Conflict {
                code: "payment_provider_reference_missing",
                message: "the Order has no payment provider reference",
            });
        }
        let checkout_details = if !is_refund {
            let order_id = aggregate_id;
            let contact = sqlx::query_as::<
                _,
                (
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ),
            >(
                "SELECT contact_email::text, contact_phone, shipping_full_name, \
                        shipping_address_line1, shipping_address_line2, shipping_locality, \
                        shipping_administrative_area, shipping_postal_code, \
                        NULLIF(btrim(shipping_country_code::text), '') \
                 FROM commerce.orders WHERE store_id = $1 AND id = $2",
            )
            .bind(job.store_id)
            .bind(order_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(corrupt_state)?;
            let shipping_address = contact
                .2
                .map(|name| {
                    let Some(line1) = contact.3 else {
                        return Err(corrupt_state());
                    };
                    let Some(city) = contact.5 else {
                        return Err(corrupt_state());
                    };
                    let Some(country_code) = contact.8 else {
                        return Err(corrupt_state());
                    };
                    Ok(PaymentShippingAddress {
                        name,
                        line1,
                        line2: contact.4,
                        city,
                        state: contact.6,
                        postal_code: contact.7,
                        country_code,
                    })
                })
                .transpose()?;
            let line_rows = sqlx::query_as::<_, (String, String, Option<String>, i32, i64)>(
                "SELECT product_title, variant_title, sku, quantity, \
                        unit_price_amount_minor \
                 FROM commerce.order_lines WHERE store_id = $1 AND order_id = $2 \
                 ORDER BY position",
            )
            .bind(job.store_id)
            .bind(order_id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(database_error)?;
            let line_items = line_rows
                .into_iter()
                .map(|line| {
                    Ok::<_, ApplicationError>(PaymentLineItem {
                        name: if line.1.trim().is_empty() {
                            line.0
                        } else {
                            format!("{} — {}", line.0, line.1)
                        },
                        sku: line.2,
                        quantity: u32::try_from(line.3).map_err(unexpected_conversion)?,
                        unit_amount_minor: line.4,
                    })
                })
                .collect::<Result<Vec<_>, ApplicationError>>()?;
            // Shipping policy is read only while creating a provider session.
            // A Cart retry with a stored client action never reaches
            // this path, so editing the Store policy cannot invalidate it.
            let shipping_countries: Vec<String> = sqlx::query_scalar(
                "SELECT country_code::text FROM commerce.store_shipping_countries \
                 WHERE store_id = $1 AND enabled ORDER BY country_code",
            )
            .bind(job.store_id)
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
                customer_email: contact.0,
                customer_phone: contact.1,
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
        let return_url = outbox_return_url(job);
        Ok(PaymentCommand {
            provider_account_id: row.2,
            kind: if is_refund {
                PaymentCommandKind::CreateRefund
            } else {
                PaymentCommandKind::CreateCheckoutSession
            },
            aggregate_id: row.8,
            refund_id: row.9,
            amount_minor: command_amount,
            currency: CurrencyCode::parse(&row.1)?,
            idempotency_key: job.id.to_string(),
            credential_secret_reference: row.3,
            provider_payment_reference: row.4,
            checkout_details,
            return_url,
            order_context: OrderMetadataContext {
                store_id: job.store_id,
                shopper_id: row.5,
                channel_id: row.6,
                order_number: row.7,
            },
        })
    }

    pub(crate) async fn record_payment_result(
        &self,
        job: &QueueJob,
        result: &PaymentCommandResult,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        if result.provider_object_id.trim().is_empty()
            || result.provider_object_id.chars().count() > 255
        {
            return Err(stripe_invalid_response());
        }
        let aggregate_id = outbox_aggregate_id(job)?;
        let internal_event_type = job
            .internal_event_type
            .as_deref()
            .ok_or_else(invalid_outbox_payload)?;
        if internal_event_type == "payment.checkout_session" {
            // Creating a Checkout Session is not a payment-information
            // submission. The payment webhook owns the eventual Purchase
            // event, so there is no analytics write at this stage.
            return Ok(());
        }
        let mut transaction = self.begin_context(None, job.store_id).await?;
        let rows = if internal_event_type == "refund.create_requested" {
            sqlx::query(
                "UPDATE commerce.order_refunds \
                 SET payment_provider_reference_id = COALESCE(payment_provider_reference_id, $3), \
                     updated_at = CASE WHEN payment_provider_reference_id IS NULL THEN $4 ELSE updated_at END \
                 WHERE store_id = $1 AND id = $2 \
                   AND (payment_provider_reference_id IS NULL OR payment_provider_reference_id = $3)",
            )
            .bind(job.store_id)
            .bind(aggregate_id)
            .bind(&result.provider_object_id)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?
            .rows_affected()
        } else {
            return Err(invalid_outbox_payload());
        };
        if rows != 1 {
            return Err(stripe_object_mismatch());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }
}

fn direct_checkout_job(
    actor: &ShopperActor,
    payment: &OrderCheckoutPayment,
    return_url: &str,
) -> QueueJob {
    QueueJob {
        id: payment.order_id.as_uuid(),
        store_id: actor.machine.store_id.as_uuid(),
        queue_name: "chaos_payment_commands".into(),
        internal_event_type: Some("payment.checkout_session".into()),
        provider_event_type: None,
        normalized_event_type: None,
        payload: json!({
            "aggregate_id": payment.order_id.as_uuid(),
            "amount_minor": payment.amount_minor,
            "currency": payment.currency.as_str(),
            "return_url": return_url,
        }),
        attempts: 1,
        provider_account_id: None,
        capability: Some("payment".into()),
        provider: Some(payment.provider.clone()),
    }
}

fn checkout_return_url(
    return_url: &str,
    order_id: OrderId,
) -> Result<String, ApplicationError> {
    let mut url = url::Url::parse(return_url).map_err(|_| invalid_outbox_payload())?;
    url.query_pairs_mut()
        .append_pair("order_id", &order_id.as_uuid().to_string());
    Ok(url.to_string())
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
