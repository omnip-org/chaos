// Payment attempt creation, retrieval, provider command handling, and payment state updates.

impl PostgresStripeRepository {
    pub(crate) async fn get_checkout_attempt_payment(
        &self,
        shopper: &ShopperActor,
        attempt_id: CheckoutAttemptId,
    ) -> Result<Option<CheckoutAttemptPayment>, ApplicationError> {
        let mut transaction = self.begin_shopper(shopper).await?;
        let detail = load_checkout_attempt_payment(
            &mut transaction,
            &shopper.machine,
            shopper.shopper_id.as_uuid(),
            attempt_id,
        )
        .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(detail)
    }

    pub(crate) async fn get_checkout_attempt(
        &self,
        shopper: &ShopperActor,
        attempt_id: CheckoutAttemptId,
    ) -> Result<Option<CheckoutAttemptDetail>, ApplicationError> {
        Ok(self
            .get_checkout_attempt_payment(shopper, attempt_id)
            .await?
            .map(|attempt| attempt.detail))
    }

    pub(crate) async fn list_checkout_attempts(
        &self,
        shopper: &ShopperActor,
    ) -> Result<Vec<CheckoutAttemptDetail>, ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                Uuid,
                Uuid,
                String,
                OffsetDateTime,
                OffsetDateTime,
                OffsetDateTime,
            ),
        >(
            "SELECT id, order_id, source_cart_id, successor_cart_id, status::text, \
                    expires_at, created_at, updated_at \
             FROM commerce.checkout_attempts \
             WHERE store_id = $1 AND sales_channel_id = $2 AND shopper_id = $3 \
               AND status IN ('creating', 'open') \
               AND expires_at > CURRENT_TIMESTAMP \
             ORDER BY created_at DESC, id DESC",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;
        let attempts = rows
            .into_iter()
            .map(|row| {
                Ok(CheckoutAttemptDetail {
                    id: CheckoutAttemptId::from_uuid(row.0),
                    order_id: OrderId::from_uuid(row.1),
                    source_cart_id: CartId::from_uuid(row.2),
                    successor_cart_id: CartId::from_uuid(row.3),
                    status: CheckoutAttemptStatus::parse(&row.4)
                        .ok_or_else(corrupt_checkout_state)?,
                    expires_at: row.5,
                    created_at: row.6,
                    updated_at: row.7,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        transaction.commit().await.map_err(database_error)?;
        Ok(attempts)
    }

    pub(crate) async fn prepare_checkout_command(
        &self,
        actor: &ShopperActor,
        attempt: &CheckoutAttemptPayment,
    ) -> Result<PaymentCommand, ApplicationError> {
        let return_url = checkout_return_url(&attempt.return_url, attempt.detail.order_id)?;
        let job = direct_checkout_job(actor, attempt, &return_url);
        let mut command = self.prepare_payment_command(&job).await?;
        command.idempotency_key = attempt.provider_idempotency_key.to_string();
        Ok(command)
    }

    pub(crate) async fn record_checkout_result(
        &self,
        shopper: &ShopperActor,
        attempt_id: CheckoutAttemptId,
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
        let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let existing = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
            "SELECT provider_session_id, provider_public_key, provider_client_secret \
             FROM commerce.checkout_attempts \
             WHERE store_id = $1 AND sales_channel_id = $2 AND shopper_id = $3 AND id = $4 \
             FOR UPDATE",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| checkout_attempt_not_found(attempt_id))?;
        if existing.0.as_deref().is_some_and(|value| value != result.provider_object_id)
            || existing
                .1
                .as_deref()
                .is_some_and(|value| value != client_action.public_key.expose_secret())
            || existing
                .2
                .as_deref()
                .is_some_and(|value| value != client_action.client_token.expose_secret())
        {
            return Err(stripe_object_mismatch());
        }
        let rows = sqlx::query(
            "UPDATE commerce.checkout_attempts \
             SET provider_session_id = $5, provider_public_key = $6, \
                 provider_client_secret = $7, \
                 status = CASE WHEN status = 'paid' THEN 'paid' ELSE 'open' END, \
                 updated_at = $8 \
             WHERE store_id = $1 AND sales_channel_id = $2 AND shopper_id = $3 AND id = $4 \
               AND status IN ('creating', 'open', 'paid')",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(&result.provider_object_id)
        .bind(client_action.public_key.expose_secret())
        .bind(client_action.client_token.expose_secret())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        if rows != 1 {
            return Err(checkout_attempt_not_open());
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub(crate) async fn fail_checkout_attempt(
        &self,
        shopper: &ShopperActor,
        attempt_id: CheckoutAttemptId,
        failure_code: &str,
        now: OffsetDateTime,
    ) -> Result<(), ApplicationError> {
        let actor = &shopper.machine;
        let channel_id = actor.sales_channel_id.ok_or(ApplicationError::Forbidden)?;
        let mut transaction = self.begin_shopper(shopper).await?;
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
            "SELECT attempt.order_id, attempt.source_cart_id, attempt.status::text, \
                    sales_order.status::text \
             FROM commerce.checkout_attempts AS attempt \
             INNER JOIN commerce.orders AS sales_order \
               ON sales_order.store_id = attempt.store_id AND sales_order.id = attempt.order_id \
             WHERE attempt.store_id = $1 AND attempt.sales_channel_id = $2 \
               AND attempt.shopper_id = $3 AND attempt.id = $4 \
             FOR UPDATE OF attempt, sales_order",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| checkout_attempt_not_found(attempt_id))?;
        let attempt_status = CheckoutAttemptStatus::parse(&row.2).ok_or_else(corrupt_checkout_state)?;
        if !matches!(attempt_status, CheckoutAttemptStatus::Creating | CheckoutAttemptStatus::Open) {
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        }
        let order_status = OrderStatus::parse(&row.3).ok_or_else(corrupt_payment_state)?;
        let rows = sqlx::query(
            "UPDATE commerce.checkout_attempts SET status = 'failed', updated_at = $5 \
             WHERE store_id = $1 AND sales_channel_id = $2 AND shopper_id = $3 AND id = $4 \
               AND status IN ('creating', 'open')",
        )
        .bind(actor.store_id.as_uuid())
        .bind(channel_id.as_uuid())
        .bind(shopper.shopper_id.as_uuid())
        .bind(attempt_id.as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?
        .rows_affected();
        if rows == 1 && order_status == OrderStatus::Pending {
            release_order_inventory(
                &mut transaction,
                actor.store_id.as_uuid(),
                row.0,
            )
            .await?;
            sqlx::query(
                "UPDATE commerce.orders SET status = 'cancelled', payment_status = 'failed', \
                        payment_failure_code = $3, updated_at = $4 \
                 WHERE store_id = $1 AND id = $2 AND status = 'pending'",
            )
            .bind(actor.store_id.as_uuid())
            .bind(row.0)
            .bind(normalize_failure_code(failure_code))
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
            sqlx::query(
                "UPDATE commerce.carts SET status = 'abandoned', version = version + 1, \
                        updated_at = $3 \
                 WHERE store_id = $1 AND id = $2 AND status = 'checkout_pending'",
            )
            .bind(actor.store_id.as_uuid())
            .bind(row.1)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;
        }
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
            _ => PaymentAttemptStatus::Failed,
        };
        // Pending refunds already claim their share of the captured amount,
        // so a second concurrent request cannot double-spend it before the
        // first one confirms via webhook.
        let already_refunded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_minor), 0)::bigint FROM commerce.refunds \
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
            "INSERT INTO commerce.refunds \
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
                        sales_order.shopper_id, sales_order.sales_channel_id, \
                        sales_order.order_number, sales_order.id, refund.id \
                 FROM commerce.refunds AS refund \
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
                        sales_order.shopper_id, sales_order.sales_channel_id, \
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
            // Checkout policy is snapshotted when the Checkout Attempt is
            // created. A later admin edit must affect new carts only; it must
            // never change the provider command for an existing attempt.
            let shipping_countries_value: Value = sqlx::query_scalar(
                "SELECT shipping_countries_snapshot FROM commerce.checkout_attempts \
                 WHERE store_id = $1 AND order_id = $2",
            )
            .bind(job.store_id)
            .bind(order_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(corrupt_state)?;
            let shipping_countries: Vec<String> =
                serde_json::from_value(shipping_countries_value).map_err(|_| corrupt_state())?;
            if shipping_countries.is_empty() {
                return Err(corrupt_state());
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
                sales_channel_id: row.6,
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
                "UPDATE commerce.refunds \
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
    attempt: &CheckoutAttemptPayment,
    return_url: &str,
) -> QueueJob {
    QueueJob {
        id: attempt.detail.id.as_uuid(),
        store_id: actor.machine.store_id.as_uuid(),
        queue_name: "chaos_payment_commands".into(),
        internal_event_type: Some("payment.checkout_session".into()),
        provider_event_type: None,
        normalized_event_type: None,
        payload: json!({
            "aggregate_id": attempt.detail.order_id.as_uuid(),
            "amount_minor": attempt.amount_minor,
            "currency": attempt.currency.as_str(),
            "return_url": return_url,
        }),
        attempts: 1,
        provider_account_id: None,
        capability: Some("payment".into()),
        provider: Some(attempt.provider.clone()),
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
        service: "stripe_checkout_client_secret",
        source: anyhow::anyhow!("Stripe Checkout Session client secret is missing"),
    }
}

fn checkout_attempt_not_found(attempt_id: CheckoutAttemptId) -> ApplicationError {
    ApplicationError::NotFound {
        resource: "checkout_attempt",
        id: attempt_id.as_uuid().to_string(),
    }
}

fn checkout_attempt_not_open() -> ApplicationError {
    ApplicationError::Conflict {
        code: "checkout_attempt_not_open",
        message: "the Checkout Attempt is no longer available for payment",
    }
}

fn normalize_failure_code(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "checkout_failed".into();
    }
    value.chars().take(2000).collect()
}
